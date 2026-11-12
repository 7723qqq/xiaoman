package dev.xiaoman.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Not exported: receives only the PendingIntents [AlarmReminders] posts. */
class ReminderReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        val text = intent.getStringExtra(EXTRA_TEXT)?.takeIf { it.isNotBlank() } ?: return
        val id = intent.getIntExtra(EXTRA_ID, 0)
        Notifications.showReminder(context, id, text)
    }

    companion object {
        const val EXTRA_TEXT = "text"
        const val EXTRA_ID = "id"
    }
}
