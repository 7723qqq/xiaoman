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
        val approvalId = intent.getLongExtra(EXTRA_APPROVAL_ID, -1L)
        when (intent.action) {
            ACTION_STOP -> {
                RustSession.nativeStop()
                Notifications.hideApproval(context)
                AgentService.stop(context)
            }
            // The Rust session refuses an id that is not the card on screen, so a stale
            // notification cannot answer a newer approval and must not hide its notification.
            ACTION_APPROVE_ONCE -> {
                if (RustSession.nativeApprove(approvalId, "once")) Notifications.hideApproval(context)
            }
            ACTION_APPROVE_TASK -> {
                if (RustSession.nativeApprove(approvalId, "task")) Notifications.hideApproval(context)
            }
            ACTION_DENY -> {
                if (RustSession.nativeDeny(approvalId)) Notifications.hideApproval(context)
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
