//! Reminders become `reminder` events carrying an RFC 5545 calendar the page can import.

use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Utc};
use serde_json::json;
use xiaoman_api::ReminderScheduler;

use crate::channels::WebChannels;
use crate::memory::Clock;
use crate::random;

/// Default zone of the server, `Asia/Shanghai` (+08:00).
pub const DEFAULT_OFFSET_MINUTES: i32 = 480;

pub struct IcsReminders {
    channels: Arc<WebChannels>,
    offset_minutes: i32,
    clock: Clock,
}

impl IcsReminders {
    pub fn new(channels: Arc<WebChannels>, offset_minutes: i32, clock: Clock) -> Self {
        Self {
            channels,
            offset_minutes,
            clock,
        }
    }
}

impl ReminderScheduler for IcsReminders {
    fn schedule(&self, text: &str, at_millis: i64) {
        self.channels.emit(json!({
            "type": "reminder",
            "text": text,
            "at": local_iso(at_millis, self.offset_minutes),
            "ics": calendar(text, at_millis, (self.clock)()),
        }));
    }
}

fn offset(offset_minutes: i32) -> FixedOffset {
    FixedOffset::east_opt(offset_minutes * 60).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap())
}

/// `2026-11-03T18:30` in the configured zone; seconds appear only when they are not zero.
pub fn local_iso(at_millis: i64, offset_minutes: i32) -> String {
    let Some(instant) = DateTime::from_timestamp_millis(at_millis) else {
        return String::new();
    };
    let local = instant.with_timezone(&offset(offset_minutes));
    use chrono::Timelike;
    if local.second() == 0 && local.nanosecond() == 0 {
        local.format("%Y-%m-%dT%H:%M").to_string()
    } else {
        local.format("%Y-%m-%dT%H:%M:%S").to_string()
    }
}

fn utc_stamp(millis: i64) -> String {
    let Some(instant) = DateTime::from_timestamp_millis(millis) else {
        return String::new();
    };
    instant
        .with_timezone(&Utc)
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

/// A one-event calendar with a display alarm, folded to 75 octets per line without splitting a
/// code point (RFC 5545).
pub fn calendar(text: &str, at_millis: i64, now_millis: i64) -> String {
    let summary = escape(text);
    let lines = [
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//Xiaoman//Web//ZH".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
        "BEGIN:VEVENT".to_string(),
        format!("UID:{}@xiaoman", random::uuid()),
        format!("DTSTAMP:{}", utc_stamp(now_millis)),
        format!("DTSTART:{}", utc_stamp(at_millis)),
        format!("SUMMARY:{summary}"),
        "BEGIN:VALARM".to_string(),
        "TRIGGER:PT0M".to_string(),
        "ACTION:DISPLAY".to_string(),
        format!("DESCRIPTION:{summary}"),
        "END:VALARM".to_string(),
        "END:VEVENT".to_string(),
        "END:VCALENDAR".to_string(),
    ];
    let folded: Vec<String> = lines.iter().map(|line| fold(line)).collect();
    format!("{}\r\n", folded.join("\r\n"))
}

fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut bytes = 0usize;
    for character in line.chars() {
        let length = character.len_utf8();
        if bytes + length > 75 {
            out.push_str("\r\n ");
            bytes = 1;
        }
        out.push(character);
        bytes += length;
    }
    out
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone};

    use super::*;

    fn millis(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        let date = NaiveDate::from_ymd_opt(year, month, day).unwrap();
        Utc.from_utc_datetime(&date.and_hms_opt(hour, minute, 0).unwrap())
            .timestamp_millis()
    }

    #[test]
    fn escapes_and_stamps_the_calendar() {
        let at = millis(2099, 11, 3, 10, 30);
        let ics = calendar("喝水,带伞;\\\n出门", at, 0);
        assert!(ics.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
        assert!(ics.ends_with("END:VCALENDAR\r\n"));
        assert!(ics.contains("DTSTART:20991103T103000Z\r\n"));
        assert!(ics.contains("SUMMARY:喝水\\,带伞\\;\\\\\\n出门\r\n"));
        assert!(ics.contains("BEGIN:VALARM\r\nTRIGGER:PT0M"));
    }

    #[test]
    fn folds_utf8_lines_without_splitting_code_points() {
        let text = "汉".repeat(100) + ",;\\\n下一行";
        let ics = calendar(&text, 0, 0);
        for line in ics.split("\r\n") {
            assert!(line.len() <= 75, "{line}");
        }
        assert!(ics.contains("DTSTART:19700101T000000Z"));
        let unfolded = ics.replace("\r\n ", "");
        assert!(unfolded.contains("\\,\\;\\\\\\n"));
    }

    #[test]
    fn local_time_uses_the_configured_zone() {
        let at = millis(2099, 11, 3, 10, 30);
        assert_eq!(local_iso(at, DEFAULT_OFFSET_MINUTES), "2099-11-03T18:30");
        assert_eq!(local_iso(at, 0), "2099-11-03T10:30");
        assert_eq!(utc_stamp(at), "20991103T103000Z");
    }
}
