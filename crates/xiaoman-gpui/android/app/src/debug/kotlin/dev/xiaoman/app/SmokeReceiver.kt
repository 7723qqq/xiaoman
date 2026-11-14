package dev.xiaoman.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import dev.xiaoman.hands.NativeLibrary
import dev.xiaoman.hands.Smoke
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The debug trigger of the scripted see-and-touch run:
 * `adb shell am broadcast -a dev.xiaoman.smoke`.
 *
 * The receiver decides nothing and logs both halves: `runSmoke` logs every step under the
 * `XiaomanSmoke` tag itself, and the summary it returns is logged here so the end of a run is
 * visible even when only this tag is filtered.
 *
 * [onReceive] runs on the main thread, and the run must not: it blocks for seconds, and
 * `captureScreen`/`dispatchStroke` refuse to run on the main thread because the main thread is what
 * delivers their platform callbacks. So the run gets a thread of its own.
 */
class SmokeReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION) return
        if (!NativeLibrary.loaded) {
            Log.w(TAG, "$ACTION: libxiaoman_android.so is not loaded")
            return
        }
        // Two registrations answer the same action: the manifest one (explicit sends) and the
        // service's runtime one. One run at a time, or two would fight over the screen.
        if (!running.compareAndSet(false, true)) {
            Log.i(TAG, "$ACTION: a run is already in progress")
            return
        }
        Thread(
            {
                try {
                    val summary = try {
                        Smoke.runSmoke()
                    } catch (e: UnsatisfiedLinkError) {
                        Log.w(TAG, "$ACTION: runSmoke is not exported by libxiaoman_android.so: ${e.message}")
                        return@Thread
                    }
                    Log.i(TAG, "$ACTION: $summary")
                } finally {
                    running.set(false)
                }
            },
            "xiaoman-smoke",
        ).start()
    }

    companion object {
        /** The action `adb shell am broadcast -a` sends; also the manifest's intent filter. */
        const val ACTION = "dev.xiaoman.smoke"

        private const val TAG = "XiaomanSmoke"

        /** True while a run is on its own thread. */
        private val running = AtomicBoolean(false)
    }
}
