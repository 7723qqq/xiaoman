package dev.xiaoman.app

import android.Manifest
import android.accessibilityservice.AccessibilityService
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import android.util.Log
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import dev.xiaoman.hands.NativeLibrary
import dev.xiaoman.hands.OverlayHost
import dev.xiaoman.hands.StatusOverlay
import dev.xiaoman.app.R
import java.lang.ref.WeakReference

/**
 * Static entry points the Rust UI calls through JNI: the foreground service, the notification
 * mirror and the status pill. Kotlin keeps the Context; Rust only names the operation.
 *
 * The pill is derived state, not another input: it is shown while `busy` and the host's activity is
 * not on screen, with the last status line Rust sent (or the app name before the first one). Rust
 * already calls [startService]/[stopService]/[updateStatus] as the session enters and leaves a run,
 * so no extra native call is needed for it.
 */
object NotifierBridge {
    private var context: Context? = null

    /** The resumed activity, for [hideApp] and [requestNotifications]; weak to not leak it. */
    @Volatile
    private var currentActivity: WeakReference<Activity>? = null

    /** Android 13+ POST_NOTIFICATIONS is asked once per process, then only from the chat banner. */
    @Volatile
    private var askedNotifications = false

    /** The last status line Rust reported; the pill keeps showing it until the next one. */
    @Volatile
    private var statusText: String? = null

    /** True between [startService] and [stopService]: a task is running. */
    @Volatile
    private var busy = false

    /** True while one of the host's activities is started; the pill only shows when it is not. */
    @Volatile
    private var uiVisible = false

    fun init(context: Context) {
        this.context = context.applicationContext
        SecretStore.init(context)
    }

    fun startService() {
        busy = true
        context?.let { AgentService.start(it) }
        syncOverlay()
    }

    fun stopService() {
        busy = false
        context?.let { AgentService.stop(it) }
        syncOverlay()
    }

    fun updateStatus(text: String?) {
        statusText = text
        context?.let { AgentService.updateStatus(it, text) }
        syncOverlay()
    }

    /** The activity lifecycle's view of "the app is on screen"; the pill would be redundant then. */
    fun setUiVisible(visible: Boolean) {
        uiVisible = visible
        syncOverlay()
    }

    /** Kept for [hideApp]/[requestNotifications]: the activity the user is looking at, if any. */
    fun onActivityResumed(activity: Activity) {
        currentActivity = WeakReference(activity)
        setUiVisible(true)
        // The Compose app asks once on its first screen, then only from the banner.
        if (!askedNotifications) {
            askedNotifications = true
            requestNotifications()
        }
    }

    fun onActivityDestroyed(activity: Activity) {
        if (currentActivity?.get() === activity) currentActivity = null
    }

    /**
     * Moves the task to the back so the agent can see the app it was operating while the operator
     * answers an approval card. Posted to the main thread: `moveTaskToBack` is a window call.
     * Returns false when there is no activity to move.
     */
    fun hideApp(): Boolean {
        val activity = currentActivity?.get() ?: return false
        activity.runOnUiThread { runCatching { activity.moveTaskToBack(true) } }
        return true
    }

    /** POST_NOTIFICATIONS on Android 13+, asked from the chat banner or on the first start. */
    fun requestNotifications() {
        val activity = currentActivity?.get() ?: return
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
        val granted = ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
        if (granted) return
        activity.runOnUiThread {
            ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), REQUEST_NOTIFICATIONS)
        }
    }

    /** The accessibility service just connected, so a running task can now show its pill. */
    fun onAccessibilityConnected() {
        syncOverlay()
    }

    /** One decrypted settings value; the Rust settings store reads the API keys through this. */
    fun readSecret(name: String): String? = SecretStore.read(name)

    fun writeSecret(name: String, value: String): Boolean = SecretStore.write(name, value)

    fun removeSecret(name: String) {
        SecretStore.remove(name)
    }

    fun showApproval(id: Long, prompt: String, reason: String) {
        Log.i("Xiaoman", "NotifierBridge.showApproval id=$id context=${context != null}")
        context?.let { Notifications.showApproval(it, id, prompt, reason) }
    }

    /** A catalog key's help page, the 去申请 button. */
    fun openUri(uri: String) {
        val app = context ?: return
        val intent = Intent(Intent.ACTION_VIEW, android.net.Uri.parse(uri))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { app.startActivity(intent) }
            .onFailure { Log.w(TAG, "openUri: ${it.message}") }
    }

    fun hideApproval() {
        context?.let { Notifications.hideApproval(it) }
    }

    fun showQuestion(text: String, takeover: Boolean) {
        context?.let { Notifications.showQuestion(it, text, takeover) }
    }

    fun hideQuestion() {
        context?.let { Notifications.hideQuestion(it) }
    }

    fun accessibilityEnabled(): Boolean = context?.let { accessibilityEnabled(it) } ?: false

    fun notificationsAllowed(): Boolean = context?.let { notificationsAllowed(it) } ?: false

    fun serviceRunning(): Boolean = AgentService.isRunning()

    fun libraryLoaded(): Boolean = NativeLibrary.loaded

    fun openAccessibilitySettings() {
        context?.startActivity(
            Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }

    fun openNotificationSettings() {
        val app = context ?: return
        app.startActivity(
            Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, app.packageName)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }

    /**
     * Brings the pill in line with the three inputs above.
     *
     * Runs on whichever thread called in (a Rust JNI thread, the main thread from the lifecycle),
     * so it serializes on this object: two threads must not create two windows.
     */
    @Synchronized
    private fun syncOverlay() {
        val service = OverlayHost.service
        if (!busy || uiVisible || service == null) {
            OverlayHost.overlay?.hide()
            return
        }
        val overlay = OverlayHost.overlay ?: createOverlay(service).also { OverlayHost.overlay = it }
        overlay.show()
        overlay.update(statusText ?: service.getString(R.string.app_name))
    }

    private fun createOverlay(service: AccessibilityService): StatusOverlay =
        StatusOverlay(service).apply {
            onStop = { stopFromOverlay(service) }
            onOpen = { openFromOverlay(service) }
        }

    /** The pill's Stop: the same stop signal the notification's action raises, then the pill goes. */
    private fun stopFromOverlay(service: Context) {
        if (NativeLibrary.loaded) {
            runCatching { RustSession.nativeStop() }
                .onFailure { Log.w(TAG, "overlay stop: ${it.message}") }
        }
        busy = false
        AgentService.stop(service)
        OverlayHost.overlay?.hide()
    }

    private fun openFromOverlay(service: Context) {
        val intent = service.packageManager.getLaunchIntentForPackage(service.packageName) ?: return
        intent.flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
        runCatching { service.startActivity(intent) }
            .onFailure { Log.w(TAG, "overlay open: ${it.message}") }
    }

    private const val TAG = "Xiaoman"
    private const val REQUEST_NOTIFICATIONS = 1
}
