//! Local reminder notifications, implemented by the platform layer.

/// Schedules a local reminder notification. The app implements it on top of AlarmManager;
/// the agent depends only on this interface and therefore stays platform-free.
pub trait ReminderScheduler: Send + Sync {
    /// Shows `text` as a notification at `at_millis` (epoch milliseconds in the device timezone).
    fn schedule(&self, text: &str, at_millis: i64);

    /// Removes the reminder for exactly `(text, at_millis)`; platforms that cannot cancel keep it.
    fn cancel(&self, _text: &str, _at_millis: i64) {}
}
