package dev.xiaoman.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * Debug-build bridge for the `xiaoman` CLI, the same actions the Kotlin product registers
 * (`app/src/debug/.../DebugReceiver.kt`):
 *
 * ```
 * adb shell am broadcast -n dev.xiaoman.host/dev.xiaoman.app.DebugReceiver \
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
            ACTION_RUN -> run(context, intent)
            ACTION_STOP -> {
                RustSession.nativeStop()
                AgentService.stop(context)
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

    private fun run(context: Context, intent: Intent) {
        val goal = intent.getStringExtra(EXTRA_GOAL)?.trim().orEmpty()
        val skill = intent.getStringExtra(EXTRA_SKILL)?.trim()?.takeIf { it.isNotEmpty() }
        val runId = intent.getStringExtra(EXTRA_RUN_ID)?.trim()?.takeIf { it.isNotEmpty() }
        if (goal.isEmpty()) {
            // A RUN without a goal still ends its run, so `--follow` returns instead of hanging.
            RustSession.nativeFail(runId ?: "debug", "missing extra 'goal'")
            Log.w(TAG, "debug: run rejected: missing extra 'goal'")
            return
        }
        AgentService.start(context)
        val started = RustSession.nativeRunTask(goal, skill, runId)
        Log.i(TAG, "debug: run ${runId ?: "-"} skill=${skill ?: "-"} started=$started")
    }

    private companion object {
        const val TAG = "Xiaoman"
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
