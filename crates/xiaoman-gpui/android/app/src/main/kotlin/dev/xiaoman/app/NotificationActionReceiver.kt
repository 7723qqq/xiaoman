package dev.xiaoman.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * Answers the approval card from the notification shade, the Kotlin `NotificationActionReceiver`:
 * allow once, allow for this task, deny, or stop the run.
 *
 * Not exported: only this app's own PendingIntents reach it.
 */
class NotificationActionReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (!RustSession.opened) return
        if (intent.action == ACTION_STOP) {
            RustSession.nativeStop()
            Notifications.hideApproval(context)
            AgentService.stop(context)
            return
        }
        // The Rust session answers only the approval whose id is on screen: a stale notification
        // cannot answer a newer one, and must not hide the newer notification either.
        val id = intent.getLongExtra(EXTRA_APPROVAL_ID, -1L)
        when (intent.action) {
            ACTION_APPROVE_ONCE -> {
                if (RustSession.nativeApprove(id, "once")) Notifications.hideApproval(context)
            }
            ACTION_APPROVE_TASK -> {
                if (RustSession.nativeApprove(id, "task")) Notifications.hideApproval(context)
            }
            ACTION_DENY -> {
                if (RustSession.nativeDeny(id)) Notifications.hideApproval(context)
            }
            else -> Log.w(TAG, "notification action: ${intent.action}")
        }
    }

    companion object {
        private const val TAG = "Xiaoman"
        const val ACTION_STOP = "dev.xiaoman.app.action.STOP"
        const val ACTION_APPROVE_ONCE = "dev.xiaoman.app.action.APPROVE_ONCE"
        const val ACTION_APPROVE_TASK = "dev.xiaoman.app.action.APPROVE_TASK"
        const val ACTION_DENY = "dev.xiaoman.app.action.DENY"
        const val EXTRA_APPROVAL_ID = "approval_id"
    }
}
