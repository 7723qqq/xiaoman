package dev.xiaoman.app.debug

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import dev.xiaoman.app.AgentService
import dev.xiaoman.app.NotifierBridge
import dev.xiaoman.app.RustSession

/**
 * Debug-build bridge for the `xiaoman` CLI, the same actions the Kotlin product registers
 * (`app/src/debug/.../DebugReceiver.kt`):
 *
 * ```
 * adb shell am broadcast -n dev.xiaoman.app/.debug.DebugReceiver \
 *   -a dev.xiaoman.debug.RUN --es goal "打开设置" --es run_id r1
 * ```
 *
 * `RUN` starts [AgentService] and hands the task to the Rust session, which emits the same
 * `Failed` / `PhoneTaskFinished` logcat lines the CLI's `--follow` waits for. `STOP`,
 * `RELOAD_SKILLS` and `SAY` map to the corresponding session calls.
 */
class DebugReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (!RustSession.opened) {
            Log.w(TAG, "debug: no session (${intent.action})")
            return
        }
        when (intent.action) {
            ACTION_RUN -> run(intent)
            ACTION_STOP -> {
                RustSession.nativeStop()
                NotifierBridge.stopService()
                Log.i(TAG, "debug: stop")
            }
            ACTION_RELOAD_SKILLS -> {
                val result = RustSession.nativeReloadSkills()
                Log.i(TAG, "debug: reload-skills ${result ?: "failed"}")
            }
            ACTION_SAY -> {
                val text = intent.getStringExtra(EXTRA_TEXT)?.trim().orEmpty()
                if (text.isEmpty()) return
                Log.i(TAG, "debug: say -> ${RustSession.send(text)}")
            }
            else -> Log.w(TAG, "debug: unknown action ${intent.action}")
        }
    }

    private fun run(intent: Intent) {
        val goal = intent.getStringExtra(EXTRA_GOAL)?.trim().orEmpty()
        val skill = intent.getStringExtra(EXTRA_SKILL)?.trim()?.takeIf { it.isNotEmpty() }
        val runId = intent.getStringExtra(EXTRA_RUN_ID)?.trim()?.takeIf { it.isNotEmpty() }
        if (goal.isEmpty()) {
            // A RUN without a goal still ends its run, so `--follow` returns instead of hanging.
            RustSession.nativeFail(runId ?: "debug", "missing extra 'goal'")
            Log.w(TAG, "debug: run rejected: missing extra 'goal'")
            return
        }
        if (runId != null && !RUN_ID.matches(runId)) {
            // Same rule as the CLI and the Kotlin DebugCommand: the id is a folder name in traces/.
            val message = "invalid run_id '$runId': use letters, digits, '.', '_' or '-'"
            RustSession.nativeFail(runId, message)
            Log.w(TAG, "debug: run rejected: $message")
            return
        }
        // A run started from the CLI is a run: the pill and the status notification follow it even
        // when the host's UI was never opened.
        NotifierBridge.startService()
        val started = RustSession.nativeRunTask(goal, skill, runId)
        Log.i(TAG, "debug: run ${runId ?: "-"} skill=${skill ?: "-"} started=$started")
    }

    private companion object {
        const val TAG = "Xiaoman"
        val RUN_ID = Regex("^[A-Za-z0-9._-]{1,80}$")
        const val ACTION_RUN = "dev.xiaoman.debug.RUN"
        const val ACTION_STOP = "dev.xiaoman.debug.STOP"
        const val ACTION_RELOAD_SKILLS = "dev.xiaoman.debug.RELOAD_SKILLS"
        const val ACTION_SAY = "dev.xiaoman.debug.SAY"
        const val EXTRA_TEXT = "text"
        const val EXTRA_GOAL = "goal"
        const val EXTRA_SKILL = "skill"
        const val EXTRA_RUN_ID = "run_id"
    }
}
