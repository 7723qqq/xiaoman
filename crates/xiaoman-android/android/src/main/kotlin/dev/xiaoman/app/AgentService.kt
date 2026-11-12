package dev.xiaoman.app

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import dev.xiaoman.hands.NativeLibrary
import dev.xiaoman.host.R

/**
 * The `specialUse` foreground service that keeps the process alive while a task runs and shows the
 * status notification with its Stop action.
 *
 * Rust reaches the four `FOREGROUND_SERVICE_METHODS` of `bridge.rs` through the static methods
 * below, so it never needs the instance. The service decides nothing: the status line comes from
 * Rust, and the Stop action calls `nativeRequestStop` back into `libxiaoman_android.so`.
 *
 * Declared `android:foregroundServiceType="specialUse"` with the same subtype explanation as the
 * Kotlin app, because this process does exactly the same thing there.
 */
class AgentService : Service() {

    /**
     * The Stop control: `MutableStopSignal::stop` on the Rust side.
     *
     * Declared as an instance native, so the JNI second argument is this service (`jobject`), not
     * the class. Nothing else about the name changes: it must not be overloaded.
     */
    external fun nativeRequestStop()

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        running = true
        createStatusChannel(this)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            requestStop()
            stopSelf(startId)
            return START_NOT_STICKY
        }
        if (!enterForeground()) {
            // The task keeps running without a notification rather than not at all.
            stopSelf(startId)
            return START_NOT_STICKY
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        running = false
        super.onDestroy()
    }

    private fun enterForeground(): Boolean = try {
        val notification = statusNotification(this, statusText)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(ID_STATUS, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(ID_STATUS, notification)
        }
        true
    } catch (e: IllegalStateException) {
        // ForegroundServiceStartNotAllowedException and friends.
        Log.w(TAG, "agent-service: cannot enter foreground: ${e.message}")
        false
    }

    private fun requestStop() {
        if (!NativeLibrary.loaded) return
        try {
            nativeRequestStop()
        } catch (e: UnsatisfiedLinkError) {
            Log.w(TAG, "nativeRequestStop is not exported by libxiaoman_android.so: ${e.message}")
        }
    }

    companion object {
        private const val TAG = "Xiaoman"

        /** Identical to `Notifier.CHANNEL_STATUS` in the Kotlin app. */
        const val CHANNEL_STATUS = "status"

        /** Identical to `Notifier.ID_STATUS`. */
        const val ID_STATUS = 1

        /** The Stop action's intent action; the notification's PendingIntent targets this service. */
        const val ACTION_STOP = "dev.xiaoman.app.AgentService.STOP"

        @Volatile
        private var running = false

        @Volatile
        private var statusText: String? = null

        /**
         * `start(Landroid/content/Context;)Z` — starts the service. False when the platform refuses
         * a background start (Android 12+); the task itself is unaffected.
         */
        @JvmStatic
        fun start(context: Context): Boolean = try {
            ContextCompat.startForegroundService(context, Intent(context, AgentService::class.java))
            true
        } catch (e: IllegalStateException) {
            Log.w(TAG, "agent-service: not started: ${e.message}")
            false
        }

        /** `stop(Landroid/content/Context;)V` */
        @JvmStatic
        fun stop(context: Context) {
            context.stopService(Intent(context, AgentService::class.java))
        }

        /** `isRunning()Z` — the service is alive and holding the process in the foreground. */
        @JvmStatic
        fun isRunning(): Boolean = running

        /**
         * `updateStatus(Landroid/content/Context;Ljava/lang/String;)V` — refreshes the status line
         * of the running notification. No-op when the service is not running or notifications are
         * not allowed.
         */
        @JvmStatic
        fun updateStatus(context: Context, status: String?) {
            statusText = status
            if (!running || !notificationsAllowed(context)) return
            val manager = context.getSystemService(NotificationManager::class.java) ?: return
            runCatching { manager.notify(ID_STATUS, statusNotification(context, status)) }
                .onFailure { Log.w(TAG, "agent-service: cannot update status: ${it.message}") }
        }

        private fun createStatusChannel(context: Context) {
            val manager = context.getSystemService(NotificationManager::class.java) ?: return
            val channel = NotificationChannel(
                CHANNEL_STATUS,
                context.getString(R.string.channel_status),
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = context.getString(R.string.channel_status_description)
                setShowBadge(false)
            }
            manager.createNotificationChannel(channel)
        }

        /** The same notification `Notifier.status` builds in the Kotlin app. */
        private fun statusNotification(context: Context, text: String?): Notification =
            NotificationCompat.Builder(context, CHANNEL_STATUS)
                .setSmallIcon(R.drawable.ic_stat_xiaoman)
                .setContentTitle(context.getString(R.string.notification_running))
                .setContentText(text)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setSilent(true)
                .setCategory(NotificationCompat.CATEGORY_SERVICE)
                .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
                .setContentIntent(openApp(context))
                .addAction(0, context.getString(R.string.chat_stop), stopAction(context))
                .build()

        private fun openApp(context: Context): PendingIntent = PendingIntent.getActivity(
            context,
            0,
            Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

        private fun stopAction(context: Context): PendingIntent = PendingIntent.getService(
            context,
            1,
            Intent(context, AgentService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

        private fun notificationsAllowed(context: Context): Boolean =
            Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
                ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
                PackageManager.PERMISSION_GRANTED
    }
}
