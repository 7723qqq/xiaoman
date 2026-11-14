package dev.xiaoman.app

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import dev.xiaoman.app.R

/**
 * The host's notifications, ported from the Kotlin `Notifier`: the status channel belongs to
 * [AgentService], this object adds the two attention channels and the reminders.
 *
 * Approval and question notifications are built here too, with the actions the Kotlin
 * `NotificationActionReceiver` handles: allow once, allow for the task, deny, stop.
 */
object Notifications {

    const val CHANNEL_ATTENTION = "attention"
    const val CHANNEL_REMINDERS = "reminders"

    fun createChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_ATTENTION,
                context.getString(R.string.channel_attention),
                NotificationManager.IMPORTANCE_HIGH,
            ).apply {
                description = context.getString(R.string.channel_attention_description)
            },
        )
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_REMINDERS,
                context.getString(R.string.channel_reminders),
                NotificationManager.IMPORTANCE_HIGH,
            ).apply {
                description = context.getString(R.string.channel_reminders_description)
            },
        )
    }

    /** The reminder alarm's notification; [id] is stable per (time, text). */
    fun showReminder(context: Context, id: Int, text: String) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        if (!notificationsAllowed(context)) return
        val notification = NotificationCompat.Builder(context, CHANNEL_REMINDERS)
            .setSmallIcon(R.drawable.ic_stat_xiaoman)
            .setContentTitle(context.getString(R.string.notification_reminder))
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setContentIntent(openApp(context))
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_REMINDER)
            .build()
        runCatching { manager.notify(REMINDER_TAG, id, notification) }
    }

    /** The approval card in the shade: the same three answers as the chat screen. */
    fun showApproval(context: Context, id: Long, prompt: String, reason: String) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        if (!notificationsAllowed(context)) return
        val notification = NotificationCompat.Builder(context, CHANNEL_ATTENTION)
            .setSmallIcon(R.drawable.ic_stat_xiaoman)
            .setContentTitle(context.getString(R.string.notification_approval))
            .setContentText(prompt)
            .setStyle(NotificationCompat.BigTextStyle().bigText(prompt))
            .setContentIntent(openApp(context))
            .setOngoing(true)
            .setCategory(NotificationCompat.CATEGORY_REMINDER)
            .addAction(0, context.getString(R.string.approval_once), action(context, id, NotificationActionReceiver.ACTION_APPROVE_ONCE, 1))
            .addAction(0, context.getString(R.string.approval_task), action(context, id, NotificationActionReceiver.ACTION_APPROVE_TASK, 2))
            .addAction(0, context.getString(R.string.approval_deny), action(context, id, NotificationActionReceiver.ACTION_DENY, 3))
            .build()
        val posted = runCatching { manager.notify(APPROVAL_TAG, APPROVAL_ID, notification) }
        Log.i("Xiaoman", "approval notification posted=${posted.isSuccess} ${posted.exceptionOrNull()?.message ?: ""}")
        // `reason` is the rule that asked; kept for the log rather than the compact notification.
        if (reason.isNotBlank()) android.util.Log.i("Xiaoman", "approval: $reason")
    }

    fun hideApproval(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        runCatching { manager.cancel(APPROVAL_TAG, APPROVAL_ID) }
    }

    /** The pending question in the shade; answering still happens in the chat. */
    fun showQuestion(context: Context, text: String, takeover: Boolean) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        if (!notificationsAllowed(context)) return
        val title = if (takeover) R.string.notification_takeover else R.string.notification_question
        val notification = NotificationCompat.Builder(context, CHANNEL_ATTENTION)
            .setSmallIcon(R.drawable.ic_stat_xiaoman)
            .setContentTitle(context.getString(title))
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setContentIntent(openApp(context))
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_REMINDER)
            .build()
        runCatching { manager.notify(QUESTION_TAG, QUESTION_ID, notification) }
    }

    fun hideQuestion(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        runCatching { manager.cancel(QUESTION_TAG, QUESTION_ID) }
    }

    /** Status notification update, forwarded from the Rust session's status line. */
    fun updateStatus(context: Context, text: String?) {
        AgentService.updateStatus(context, text)
    }

    private fun action(context: Context, approvalId: Long, action: String, request: Int): PendingIntent {
        val intent = Intent(context, NotificationActionReceiver::class.java).apply {
            this.action = action
            putExtra(NotificationActionReceiver.EXTRA_APPROVAL_ID, approvalId)
        }
        return PendingIntent.getBroadcast(
            context,
            request,
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
    }

    private fun openApp(context: Context): PendingIntent {
        val intent = (context.packageManager.getLaunchIntentForPackage(context.packageName) ?: Intent()).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
        }
        return PendingIntent.getActivity(
            context,
            0,
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
    }

    private fun notificationsAllowed(context: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED

    private const val REMINDER_TAG = "reminder"
    private const val APPROVAL_TAG = "approval"
    private const val APPROVAL_ID = 2
    private const val QUESTION_TAG = "question"
    private const val QUESTION_ID = 3
}
