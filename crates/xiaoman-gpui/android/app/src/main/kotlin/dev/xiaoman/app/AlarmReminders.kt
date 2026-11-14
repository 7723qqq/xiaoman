package dev.xiaoman.app

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * `reminder_set` on Android, the Kotlin `AlarmReminderScheduler`: one inexact
 * `setAndAllowWhileIdle` alarm per reminder, which also fires in Doze and needs no exact-alarm
 * permission. Reboot clears the alarms, exactly like the Kotlin product.
 *
 * Called from Rust through `AlarmReminders.schedule/cancel(String, long)` (see
 * `crates/xiaoman-android/src/session.rs`), so both methods must stay `@JvmStatic`.
 */
object AlarmReminders {

    private const val TAG = "Xiaoman"
    private var context: Context? = null

    fun init(context: Context) {
        this.context = context.applicationContext
    }

    @JvmStatic
    fun schedule(text: String, atMillis: Long) {
        val app = context ?: run {
            Log.w(TAG, "reminders: not initialized")
            return
        }
        val alarms = app.getSystemService(AlarmManager::class.java) ?: return
        val id = reminderId(text, atMillis)
        val intent = Intent(app, ReminderReceiver::class.java).apply {
            putExtra(ReminderReceiver.EXTRA_ID, id)
            putExtra(ReminderReceiver.EXTRA_TEXT, text)
        }
        val pending = PendingIntent.getBroadcast(
            app,
            id,
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        alarms.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, atMillis, pending)
        Log.i(TAG, "reminders: scheduled '$text' at $atMillis")
    }

    /** Removes one alarm previously scheduled with the same text and time. */
    @JvmStatic
    fun cancel(text: String, atMillis: Long) {
        val app = context ?: run {
            Log.w(TAG, "reminders: not initialized")
            return
        }
        val alarms = app.getSystemService(AlarmManager::class.java) ?: return
        val id = reminderId(text, atMillis)
        val intent = Intent(app, ReminderReceiver::class.java).apply {
            putExtra(ReminderReceiver.EXTRA_ID, id)
            putExtra(ReminderReceiver.EXTRA_TEXT, text)
        }
        val pending = PendingIntent.getBroadcast(
            app,
            id,
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_NO_CREATE,
        )
        if (pending != null) {
            alarms.cancel(pending)
            pending.cancel()
            Log.i(TAG, "reminders: cancelled '$text' at $atMillis")
        }
    }

    /** Stable per (time, text): identical reminders collapse, different texts at one minute don't. */
    private fun reminderId(text: String, atMillis: Long): Int =
        (atMillis.toInt() xor text.hashCode()) and Int.MAX_VALUE
}
