package dev.xiaoman.hands

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.ActivityNotFoundException
import android.content.ComponentName
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Path
import android.graphics.Rect
import android.hardware.display.DisplayManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.AlarmClock
import android.util.DisplayMetrics
import android.util.Log
import android.view.Display
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityNodeInfo.AccessibilityAction
import android.view.accessibility.AccessibilityWindowInfo
import dev.xiaoman.app.NotifierBridge
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlin.math.max
import kotlin.math.roundToInt

/**
 * The `AccessibilityService` the user enables in system Settings: the only object allowed to call
 * accessibility APIs, and a thin adapter between Android objects and the Rust core.
 *
 * Every public method here is one row of `SERVICE_METHODS` in
 * `crates/xiaoman-android/src/bridge.rs`, in the same order. Every `external fun` is one row of
 * `SERVICE_NATIVES`. Nothing decides anything: this class reads Android objects into the JSON
 * shapes the contract names, keeps opaque node handles, and calls back into `libxiaoman_android.so`.
 *
 * Threading, as `bridge.rs` requires: Rust calls these methods from its own runtime threads and
 * never posts to a `Looper`, so anything that touches a window or the display hops to the main
 * thread here. Blocking calls wait on the calling Rust thread, never on the main thread.
 *
 * Node handles: a handle is a token for one `AccessibilityNodeInfo` in [nodes]. Every non-zero
 * handle is released exactly once by Rust; this class only keeps the table and returns the nodes to
 * the platform pool when it is released.
 */
class XiaomanAccessibilityService : AccessibilityService() {

    // ------------------------------------------------------------------ natives (SERVICE_NATIVES)
    //
    // Exported by libxiaoman_android.so as
    // Java_dev_xiaoman_hands_XiaomanAccessibilityService_<name>. They must not be overloaded: an
    // overload would need the JNI signature suffix.

    /** `onServiceConnected` handed Rust the live service object; Rust keeps it as a global ref. */
    private external fun nativeOnServiceConnected()

    /** `onUnbind`/`onDestroy`: Rust drops the global ref and reports the device as disconnected. */
    private external fun nativeOnServiceUnbound()

    /**
     * The Stop control's entry point: `MutableStopSignal::stop` on the Rust side.
     *
     * Nothing in this shell shows a Stop control (the foreground notification does), so today only
     * the notification's stop action and the status screen reach Rust. The method stays public so a
     * future overlay can call it without a second native.
     */
    external fun nativeRequestStop()

    /** A `TYPE_WINDOW_STATE_CHANGED` event, so Rust knows the foreground app between observations. */
    private external fun nativeOnForeground(packageName: String?, activityName: String?)

    // ------------------------------------------------------------------ state

    /** `0` is "no node"; handles start at 1 so `NodeHandle::NULL` always round-trips. */
    private val nextHandle = AtomicLong(1)

    private val nodes = ConcurrentHashMap<Long, AccessibilityNodeInfo>()

    @Volatile
    private var connected = false

    @Volatile
    private var foregroundPkg: String? = null

    @Volatile
    private var foregroundCls: String? = null

    /** Activity-name lookups are cached; window-state events are frequent. */
    private val activityCache = ConcurrentHashMap<ComponentName, Boolean>()

    private val mainHandler = Handler(Looper.getMainLooper())

    private val shotLock = Any()
    private var lastShotAtMs = 0L

    // ------------------------------------------------------------------ lifecycle

    override fun onServiceConnected() {
        super.onServiceConnected()
        connected = true
        callback("nativeOnServiceConnected") { nativeOnServiceConnected() }
        // The pill can only be added from the connected service; a task may already be running.
        OverlayHost.service = this
        NotifierBridge.onAccessibilityConnected()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event == null || event.eventType != AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED) return
        val pkg = event.packageName?.toString() ?: return
        val cls = event.className?.toString()
        if (cls != null && isActivity(ComponentName(pkg, cls))) {
            setForeground(pkg, cls)
        } else if (foregroundPkg != pkg) {
            // A window of a new app whose activity we cannot resolve (package not visible, dialog,
            // custom window). Only switch when it really is the active window, not a toast or the IME.
            val root = rootInActiveWindow
            val rootPkg = root?.packageName?.toString()
            recycle(root)
            if (rootPkg == pkg) setForeground(pkg, null)
        }
    }

    override fun onInterrupt() = Unit

    override fun onUnbind(intent: Intent?): Boolean {
        disconnect()
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        disconnect()
        super.onDestroy()
    }

    private fun setForeground(packageName: String, activityName: String?) {
        foregroundPkg = packageName
        foregroundCls = activityName
        callback("nativeOnForeground") { nativeOnForeground(packageName, activityName) }
    }

    private fun isActivity(component: ComponentName): Boolean = activityCache.getOrPut(component) {
        try {
            packageManager.getActivityInfo(component, 0)
            true
        } catch (_: PackageManager.NameNotFoundException) {
            false
        }
    }

    private fun disconnect() {
        if (!connected) return
        connected = false
        releaseAll()
        // The pill's window dies with the service; the notifier recreates it on the next connect.
        OverlayHost.overlay?.hide()
        OverlayHost.overlay = null
        OverlayHost.service = null
        // Keep the last foreground app: Rust reports the device as disconnected until the next connect.
        callback("nativeOnServiceUnbound") { nativeOnServiceUnbound() }
    }

    // ------------------------------------------------------------------ SERVICE_METHODS
    // Same order and descriptors as the table in bridge.rs.

    /** `()Z` — the user has the service enabled and it is connected. */
    fun isConnected(): Boolean = connected

    /** `()I` — display width in pixels, in the current orientation. */
    fun displayWidthPx(): Int = realMetrics().widthPixels

    /** `()I` — display height in pixels, in the current orientation. */
    fun displayHeightPx(): Int = realMetrics().heightPixels

    /** `()Ljava/lang/String;` — package of the last `TYPE_WINDOW_STATE_CHANGED`, or null. */
    fun foregroundPackage(): String? = foregroundPkg

    /** `()Ljava/lang/String;` — resolvable activity of that window, or null. */
    fun foregroundActivity(): String? = foregroundCls

    /**
     * `()Ldev/xiaoman/hands/ScreenShot;` — one PNG downscaled to a 1280 px long edge, or no bytes
     * and `secure` when the window is `FLAG_SECURE` or was captured as an all-black frame.
     *
     * Blocks for up to about a second: Android allows one screenshot per second per service, and
     * the caller (a Rust runtime thread) waits for the interval slot and for the callback.
     */
    fun captureScreen(): ScreenShot = synchronized(shotLock) {
        // Blocking here waits for an interval slot and for the platform callback: never on main.
        if (Looper.myLooper() == Looper.getMainLooper()) {
            Log.w(TAG, "captureScreen must not be called on the main thread")
            return ScreenShot(null, false)
        }
        // The pill must not appear in the screenshot, and its window must not eat a capture.
        val overlay = OverlayHost.overlay
        overlay?.hideForCapture()
        try {
            repeat(SCREENSHOT_ATTEMPTS) {
                waitForShotSlot()
                val result = takeShot()
                lastShotAtMs = SystemClock.uptimeMillis()
                when (result) {
                    is RawShot.Ok -> return encode(result.bitmap)
                    is RawShot.Failed -> when (result.code) {
                        // Another capture took the slot; wait it out and try again.
                        AccessibilityService.ERROR_TAKE_SCREENSHOT_INTERVAL_TIME_SHORT -> Unit
                        AccessibilityService.ERROR_TAKE_SCREENSHOT_SECURE_WINDOW -> {
                            Log.i(TAG, "captureScreen: the window is FLAG_SECURE")
                            return ScreenShot(null, true)
                        }
                        else -> {
                            Log.w(TAG, "takeScreenshot failed: ${result.code}")
                            return ScreenShot(null, false)
                        }
                    }
                }
            }
            ScreenShot(null, false)
        } finally {
            overlay?.restoreAfterCapture()
        }
    }

    /**
     * `()[J` — active window's root first, then the other application and focused-system windows
     * top-down (dialogs, permission prompts). The IME, the status and navigation bars and
     * Xiaoman's own accessibility overlay are skipped.
     */
    fun readRoots(): LongArray {
        val roots = onMainValue { collectRoots() } ?: return LongArray(0)
        if (roots.isEmpty()) return LongArray(0)
        return LongArray(roots.size) { adopt(roots[it]) }
    }

    /** `(J)Ljava/lang/String;` — the node facts JSON of `bridge.rs`, or null for a dead handle. */
    fun nodeFactsJson(handle: Long): String? {
        val node = nodeOf(handle) ?: return null
        return runCatching { factsJson(node) }.onFailure { Log.w(TAG, "facts of $handle", it) }.getOrNull()
    }

    /** `(J)I` */
    fun nodeChildCount(handle: Long): Int =
        nodeOf(handle)?.let { runCatching { it.childCount }.getOrDefault(0) } ?: 0

    /** `(JI)J` — `0` when the child vanished between `nodeChildCount` and this call. */
    fun nodeChild(handle: Long, index: Int): Long {
        if (index < 0) return 0L
        val parent = nodeOf(handle) ?: return 0L
        val child = runCatching { parent.getChild(index) }.getOrNull() ?: return 0L
        return adopt(child)
    }

    /** `(J)V` — returns one node to the platform pool. Handles are released exactly once by Rust. */
    fun nodeRelease(handle: Long) {
        recycle(nodes.remove(handle))
    }

    /** `(J)Z` — re-reads the live node; false when it is stale. */
    fun nodeRefresh(handle: Long): Boolean =
        nodeOf(handle)?.let { runCatching { it.refresh() }.getOrDefault(false) } ?: false

    /** `(J)Z` — `ACTION_CLICK` on the node. Rust taps the node's centre when this returns false. */
    fun nodeClick(handle: Long): Boolean = perform(handle, AccessibilityNodeInfo.ACTION_CLICK)

    /** `(J)Z` — `ACTION_LONG_CLICK` on the node. */
    fun nodeLongClick(handle: Long): Boolean = perform(handle, AccessibilityNodeInfo.ACTION_LONG_CLICK)

    /** `(JLjava/lang/String;)Z` — `ACTION_SET_TEXT` with the whole text. */
    fun nodeSetText(handle: Long, text: String?): Boolean {
        if (text == null) return false
        val target = nodeOf(handle) ?: return false
        val args = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
        }
        return runCatching { target.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args) }
            .getOrDefault(false)
    }

    /** `(J)Z` — `ACTION_IME_ENTER`, the submit after `nodeSetText`. */
    fun nodeImeEnter(handle: Long): Boolean = perform(handle, AccessibilityAction.ACTION_IME_ENTER.id)

    /** `()J` — the focused editable node when the model did not name one, or `0`. */
    fun editableFocus(): Long {
        val focused = onMainValue { findFocus(AccessibilityNodeInfo.FOCUS_INPUT) } ?: return 0L
        if (runCatching { focused.isEditable }.getOrDefault(false)) return adopt(focused)
        recycle(focused)
        return 0L
    }

    /**
     * `([FJ)Z` — one gesture, `[x0, y0, x1, y1, …]` in screen pixels, held for `durationMs`.
     *
     * Blocks until `GestureResultCallback` reports completion or cancellation, so Rust sees one
     * boolean per gesture. The gesture is dispatched on the main thread and the callback is
     * delivered to the main thread; the waiting thread is the Rust caller, never the main thread.
     */
    fun dispatchStroke(points: FloatArray, durationMs: Long): Boolean {
        if (points.isEmpty() || points.size % 2 != 0) return false
        // Blocking here waits for the gesture callback: never on main.
        if (Looper.myLooper() == Looper.getMainLooper()) {
            Log.w(TAG, "dispatchStroke must not be called on the main thread")
            return false
        }
        val duration = durationMs.coerceAtLeast(1L)
        // The pill must not swallow the gesture it might sit under.
        val overlay = OverlayHost.overlay
        overlay?.hideForCapture()
        try {
            val path = Path().apply {
                moveTo(points[0], points[1])
                var i = 2
                while (i + 1 < points.size) {
                    lineTo(points[i], points[i + 1])
                    i += 2
                }
            }
            val gesture = try {
                GestureDescription.Builder()
                    .addStroke(GestureDescription.StrokeDescription(path, 0, duration))
                    .build()
            } catch (e: IllegalArgumentException) {
                Log.w(TAG, "cannot build gesture", e)
                return false
            }
            val done = CountDownLatch(1)
            val completed = AtomicBoolean(false)
            val callback = object : AccessibilityService.GestureResultCallback() {
                override fun onCompleted(gestureDescription: GestureDescription?) {
                    completed.set(true)
                    done.countDown()
                }

                override fun onCancelled(gestureDescription: GestureDescription?) {
                    completed.set(false)
                    done.countDown()
                }
            }
            post {
                val accepted = runCatching { dispatchGesture(gesture, callback, mainHandler) }.getOrDefault(false)
                if (!accepted) {
                    completed.set(false)
                    done.countDown()
                }
            }
            return if (done.await(duration + GESTURE_TIMEOUT_SLACK_MS, TimeUnit.MILLISECONDS)) completed.get() else false
        } finally {
            overlay?.restoreAfterCapture()
        }
    }

    /** `()Z` — `GLOBAL_ACTION_BACK`. */
    fun globalBack(): Boolean = global(AccessibilityService.GLOBAL_ACTION_BACK)

    /** `()Z` — `GLOBAL_ACTION_HOME`. */
    fun globalHome(): Boolean = global(AccessibilityService.GLOBAL_ACTION_HOME)

    /** `(Ljava/lang/String;)Z` — launcher intent plus `FLAG_ACTIVITY_NEW_TASK`. */
    fun launch(packageName: String): Boolean {
        val intent = runCatching { packageManager.getLaunchIntentForPackage(packageName) }.getOrNull()
            ?: return false
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return start(intent)
    }

    /** `(Ljava/lang/String;Ljava/lang/String;)Z` — `ACTION_VIEW`, restricted to a package when given. */
    fun openUri(uri: String, packageName: String?): Boolean {
        val parsed = runCatching { Uri.parse(uri) }.getOrNull() ?: return false
        if (parsed.scheme.isNullOrEmpty()) return false
        val intent = Intent(Intent.ACTION_VIEW, parsed).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (!packageName.isNullOrEmpty()) intent.setPackage(packageName)
        return start(intent)
    }

    /**
     * `(IILjava/lang/String;)Z` — `ACTION_SET_ALARM` with the extras of `app_links.rs`. The clock UI
     * is never skipped: the operator checks the prefilled page and taps save.
     */
    fun startAlarm(hour: Int, minute: Int, label: String?): Boolean {
        if (hour !in 0..23 || minute !in 0..59) return false
        val intent = Intent(AlarmClock.ACTION_SET_ALARM)
            .putExtra(AlarmClock.EXTRA_HOUR, hour)
            .putExtra(AlarmClock.EXTRA_MINUTES, minute)
            .putExtra(AlarmClock.EXTRA_SKIP_UI, false)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (!label.isNullOrEmpty()) intent.putExtra(AlarmClock.EXTRA_MESSAGE, label)
        return start(intent)
    }

    /** `()Ljava/lang/String;` — `[{"packageName":…,"label":…}]`, one per package, sorted by label. */
    fun installedAppsJson(): String = runCatching {
        val pm = packageManager
        val intent = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        @Suppress("DEPRECATION")
        val activities = pm.queryIntentActivities(intent, 0)
        val seen = HashSet<String>()
        val entries = activities.mapNotNull { info ->
            val pkg = info.activityInfo?.packageName ?: return@mapNotNull null
            if (!seen.add(pkg)) return@mapNotNull null
            val label = runCatching { info.loadLabel(pm).toString() }.getOrNull().orEmpty().ifBlank { pkg }
            pkg to label
        }.sortedBy { it.second }
        val array = JSONArray()
        for ((pkg, label) in entries) {
            array.put(JSONObject().put("packageName", pkg).put("label", label))
        }
        array.toString()
    }.getOrElse {
        Log.w(TAG, "installed apps", it)
        "[]"
    }

    // ------------------------------------------------------------------ node facts

    private fun factsJson(node: AccessibilityNodeInfo): String {
        val bounds = Rect()
        node.getBoundsInScreen(bounds)
        return JSONObject()
            .put("text", node.text?.toString() ?: JSONObject.NULL)
            .put("contentDescription", node.contentDescription?.toString() ?: JSONObject.NULL)
            // Raw hint text: Rust resolves description from contentDescription and hintText itself.
            .put("hintText", node.hintText?.toString() ?: JSONObject.NULL)
            .put("resourceId", node.viewIdResourceName ?: JSONObject.NULL)
            .put("className", node.className?.toString() ?: JSONObject.NULL)
            .put("bounds", JSONArray().put(bounds.left).put(bounds.top).put(bounds.right).put(bounds.bottom))
            .put("visible", node.isVisibleToUser)
            .put("clickable", node.isClickable)
            .put("longClickable", node.isLongClickable)
            .put("editable", node.isEditable)
            .put("scrollable", node.isScrollable)
            .put("focused", node.isFocused)
            .put("password", node.isPassword)
            .toString()
    }

    private fun perform(handle: Long, action: Int): Boolean =
        nodeOf(handle)?.let { runCatching { it.performAction(action) }.getOrDefault(false) } ?: false

    private fun global(action: Int): Boolean =
        onMainValue { performGlobalAction(action) } ?: false

    private fun nodeOf(handle: Long): AccessibilityNodeInfo? = nodes[handle]

    private fun adopt(node: AccessibilityNodeInfo): Long {
        val handle = nextHandle.getAndIncrement()
        nodes[handle] = node
        return handle
    }

    private fun releaseAll() {
        val released = ArrayList(nodes.values)
        nodes.clear()
        released.forEach { recycle(it) }
    }

    /** `recycle()` is a no-op (and deprecated) from API 33; before that it returns the node to the pool. */
    @Suppress("DEPRECATION")
    private fun recycle(node: AccessibilityNodeInfo?) {
        if (node == null) return
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) runCatching { node.recycle() }
    }

    // ------------------------------------------------------------------ windows and display

    /** Caller hops to the main thread; see [readRoots]. */
    private fun collectRoots(): List<AccessibilityNodeInfo> {
        val roots = ArrayList<AccessibilityNodeInfo>(4)
        val windows = runCatching { windows }.getOrDefault(emptyList())
        val activeRoot = rootInActiveWindow
        val activeWindow = activeRoot?.let { root -> windows.firstOrNull { it.id == root.windowId } }
        // A system window in front is usable; an IME or an accessibility overlay is not a screen.
        val activeUsable = activeWindow == null ||
            activeWindow.type == AccessibilityWindowInfo.TYPE_APPLICATION ||
            activeWindow.type == AccessibilityWindowInfo.TYPE_SYSTEM
        val active = if (activeUsable) {
            activeRoot
        } else {
            recycle(activeRoot)
            null
        }
        if (active != null) roots += active
        for (window in windows.sortedByDescending { it.layer }) {
            if (window.type != AccessibilityWindowInfo.TYPE_APPLICATION &&
                window.type != AccessibilityWindowInfo.TYPE_SYSTEM
            ) {
                continue
            }
            if (active != null && window.id == active.windowId) continue
            // System windows: permission and system dialogs only, not the status or navigation bar.
            if (window.type == AccessibilityWindowInfo.TYPE_SYSTEM && !window.isFocused) continue
            window.root?.let { roots += it }
        }
        return roots
    }

    private fun realMetrics(): DisplayMetrics {
        val display = getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        val metrics = DisplayMetrics()
        @Suppress("DEPRECATION")
        display.getRealMetrics(metrics)
        return metrics
    }

    // ------------------------------------------------------------------ screenshots

    private sealed interface RawShot {
        class Ok(val bitmap: Bitmap) : RawShot
        class Failed(val code: Int) : RawShot
    }

    /** Waits out the rest of Android's one-screenshot-per-second allowance. */
    private fun waitForShotSlot() {
        val wait = lastShotAtMs + SCREENSHOT_INTERVAL_MS - SystemClock.uptimeMillis()
        if (wait > 0) {
            try {
                Thread.sleep(wait)
            } catch (e: InterruptedException) {
                Thread.currentThread().interrupt()
            }
        }
    }

    /**
     * One `takeScreenshot` call, blocking the calling thread until the callback arrives. The API
     * call itself is made on the main thread; nothing waits on the main thread.
     */
    private fun takeShot(): RawShot {
        val done = CountDownLatch(1)
        val holder = Box<RawShot>()
        val callback = object : AccessibilityService.TakeScreenshotCallback {
            override fun onSuccess(result: AccessibilityService.ScreenshotResult) {
                val shot = try {
                    val buffer = result.hardwareBuffer
                    try {
                        val bitmap = Bitmap.wrapHardwareBuffer(buffer, result.colorSpace)?.let { hardware ->
                            hardware.copy(Bitmap.Config.ARGB_8888, false).also { hardware.recycle() }
                        }
                        if (bitmap != null) RawShot.Ok(bitmap) else RawShot.Failed(AccessibilityService.ERROR_TAKE_SCREENSHOT_INTERNAL_ERROR)
                    } finally {
                        buffer.close()
                    }
                } catch (e: RuntimeException) {
                    Log.w(TAG, "screenshot buffer", e)
                    RawShot.Failed(AccessibilityService.ERROR_TAKE_SCREENSHOT_INTERNAL_ERROR)
                }
                holder.value = shot
                done.countDown()
            }

            override fun onFailure(errorCode: Int) {
                holder.value = RawShot.Failed(errorCode)
                done.countDown()
            }
        }
        post {
            val accepted = runCatching {
                takeScreenshot(Display.DEFAULT_DISPLAY, DIRECT_EXECUTOR, callback)
                true
            }.getOrElse { e ->
                Log.w(TAG, "takeScreenshot threw", e)
                false
            }
            if (!accepted) {
                holder.value = RawShot.Failed(AccessibilityService.ERROR_TAKE_SCREENSHOT_INTERNAL_ERROR)
                done.countDown()
            }
        }
        if (!done.await(SCREENSHOT_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
            Log.w(TAG, "takeScreenshot timed out")
            return RawShot.Failed(AccessibilityService.ERROR_TAKE_SCREENSHOT_INTERNAL_ERROR)
        }
        return holder.value ?: RawShot.Failed(AccessibilityService.ERROR_TAKE_SCREENSHOT_INTERNAL_ERROR)
    }

    /**
     * Downscales to [DEFAULT_LONG_EDGE] and PNG-encodes, like `geometry::target_size` and
     * `AccessibilityDevice.encode`. An all-black frame means a `FLAG_SECURE` window on Android
     * 11–13, which reports success instead of `ERROR_TAKE_SCREENSHOT_SECURE_WINDOW`.
     */
    private fun encode(full: Bitmap): ScreenShot {
        try {
            val (tw, th) = targetSize(full.width, full.height)
            val scaled = if (tw == full.width && th == full.height) full else Bitmap.createScaledBitmap(full, tw, th, true)
            try {
                val pixels = IntArray(scaled.width * scaled.height)
                scaled.getPixels(pixels, 0, scaled.width, 0, 0, scaled.width, scaled.height)
                if (looksAllBlack(pixels, scaled.width, scaled.height)) {
                    Log.i(TAG, "captureScreen: the frame is all black (${scaled.width}x${scaled.height})")
                    return ScreenShot(null, true)
                }
                val out = ByteArrayOutputStream(256 * 1024)
                scaled.compress(Bitmap.CompressFormat.PNG, 100, out)
                return ScreenShot(out.toByteArray(), false)
            } finally {
                if (scaled !== full) scaled.recycle()
            }
        } finally {
            full.recycle()
        }
    }

    // ------------------------------------------------------------------ main-thread hops

    /** Runs on the main thread and returns the value, or null when the main thread did not answer. */
    private fun <T> onMainValue(block: () -> T): T? {
        if (Looper.myLooper() == Looper.getMainLooper()) return block()
        val done = CountDownLatch(1)
        val holder = Box<T>()
        mainHandler.post {
            try {
                holder.value = block()
            } catch (e: RuntimeException) {
                Log.w(TAG, "main-thread call failed", e)
            } finally {
                done.countDown()
            }
        }
        if (!done.await(MAIN_HOP_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
            Log.w(TAG, "main thread did not answer")
            return null
        }
        return holder.value
    }

    /** Hands work to the main thread; returns immediately. */
    private fun post(block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) block() else mainHandler.post { block() }
    }

    // ------------------------------------------------------------------ helpers

    private fun start(intent: Intent): Boolean = try {
        startActivity(intent)
        true
    } catch (e: ActivityNotFoundException) {
        Log.w(TAG, "no app can open it: ${e.message}")
        false
    } catch (e: SecurityException) {
        Log.w(TAG, "not allowed: ${e.message}")
        false
    }

    /**
     * Calls a Rust callback. A library that is missing or exports no such symbol is logged rather
     * than thrown at the system, which would crash the service while the user tries to enable it.
     */
    private fun callback(what: String, block: () -> Unit) {
        if (!NativeLibrary.loaded) return
        try {
            block()
        } catch (e: UnsatisfiedLinkError) {
            Log.w(TAG, "$what is not exported by libxiaoman_android.so: ${e.message}")
        }
    }

    /** A value handed between threads; the latch provides the happens-before edge. */
    private class Box<T>(var value: T? = null)

    private companion object {
        const val TAG = "Xiaoman"

        /** Android's own limit is one screenshot per second; leave a margin for the callback. */
        const val SCREENSHOT_INTERVAL_MS = 1050L
        const val SCREENSHOT_ATTEMPTS = 3
        const val SCREENSHOT_TIMEOUT_MS = 5_000L
        const val GESTURE_TIMEOUT_SLACK_MS = 5_000L
        const val MAIN_HOP_TIMEOUT_MS = 5_000L

        /** `geometry::DEFAULT_LONG_EDGE`. */
        const val DEFAULT_LONG_EDGE = 1280

        /** `geometry::DEFAULT_BLACK_SAMPLES`: the grid `looks_all_black` samples. */
        const val BLACK_SAMPLES = 16

        val DIRECT_EXECUTOR: Executor = Executor { it.run() }

        /** `geometry::target_size`: long edge at most [DEFAULT_LONG_EDGE], never upscaled. */
        fun targetSize(width: Int, height: Int): Pair<Int, Int> {
            if (width <= 0 || height <= 0) return 0 to 0
            val longEdge = max(width, height)
            if (longEdge <= DEFAULT_LONG_EDGE) return width to height
            val scale = DEFAULT_LONG_EDGE.toDouble() / longEdge
            val scaledWidth = max(1, (width * scale).roundToInt())
            val scaledHeight = max(1, (height * scale).roundToInt())
            return scaledWidth to scaledHeight
        }

        /** `geometry::looks_all_black`: every sampled pixel is opaque pure black. */
        fun looksAllBlack(pixels: IntArray, width: Int, height: Int): Boolean {
            if (width <= 0 || height <= 0 || pixels.size < width * height) return false
            for (sampleY in 0 until BLACK_SAMPLES) {
                val y = sampleCoord(sampleY, height)
                for (sampleX in 0 until BLACK_SAMPLES) {
                    val x = sampleCoord(sampleX, width)
                    if ((pixels[y * width + x] and 0x00FFFFFF) != 0) return false
                }
            }
            return true
        }

        fun sampleCoord(index: Int, extent: Int): Int =
            ((index + 0.5) * extent / BLACK_SAMPLES).toInt().coerceAtMost(extent - 1)
    }
}
