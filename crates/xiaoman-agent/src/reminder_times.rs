//! Parsing and formatting of the reminder tool's local times; the device timezone is used throughout.

use chrono::{Datelike, Local, NaiveDateTime, TimeZone, Weekday};

pub struct ReminderTimes;

impl ReminderTimes {
    /// Parses an ISO-8601 local time such as "2026-11-03T08:00" to epoch millis, or `None`.
    pub fn parse_at_millis(raw: &str) -> Option<i64> {
        let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
            .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M"))
            .ok()?;
        Local
            .from_local_datetime(&naive)
            .single()
            .map(|time| time.timestamp_millis())
    }

    /// Current device-local time for the chat model's system prompt, e.g. "2026-11-02 07:50 星期一".
    pub fn prompt_now(now_millis: i64) -> String {
        let time = Local
            .timestamp_millis_opt(now_millis)
            .single()
            .unwrap_or_else(Local::now);
        format!(
            "{} {}",
            time.format("%Y-%m-%d %H:%M"),
            weekday_cn(time.weekday())
        )
    }
}

fn weekday_cn(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "星期一",
        Weekday::Tue => "星期二",
        Weekday::Wed => "星期三",
        Weekday::Thu => "星期四",
        Weekday::Fri => "星期五",
        Weekday::Sat => "星期六",
        Weekday::Sun => "星期日",
    }
}
