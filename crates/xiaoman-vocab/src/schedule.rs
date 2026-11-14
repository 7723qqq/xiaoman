//! Date arithmetic and the Ebbinghaus intervals. Dates are local `YYYY-MM-DD` strings.

use chrono::{Datelike, Duration, FixedOffset, Local, NaiveDate, TimeZone, Utc};
use xiaoman_api::VocabError;

/// Reviews after the first study day: +1, +2, +4, +7 and +15 days.
pub const REVIEW_INTERVALS: [i64; 5] = [1, 2, 4, 7, 15];
pub const MASTERED_STAGE: u8 = 5;

pub fn parse_date(raw: &str) -> Result<NaiveDate, VocabError> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")
        .map_err(|_| VocabError(format!("日期格式应为 YYYY-MM-DD，收到「{}」", raw.trim())))
}

pub fn format_date(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

pub fn add_days(date: NaiveDate, days: i64) -> String {
    format_date(date + Duration::days(days))
}

/// The stage and due date after a review on `today`.
///
/// A wrong answer resets the word to stage 0 and brings it back tomorrow. A correct answer moves
/// one stage forward and adds that stage's interval; stage 5 means mastered with no due date.
pub fn review_result(today: NaiveDate, stage: u8, correct: bool) -> (u8, Option<String>) {
    if !correct {
        return (0, Some(add_days(today, 1)));
    }
    let next = (stage + 1).min(MASTERED_STAGE);
    if next >= MASTERED_STAGE {
        (MASTERED_STAGE, None)
    } else {
        (next, Some(add_days(today, REVIEW_INTERVALS[next as usize])))
    }
}

/// Parses `HH:MM` and returns the normalized `HH:MM` plus its hour and minute.
pub fn parse_reminder_time(raw: &str) -> Result<(String, u32, u32), VocabError> {
    let text = raw.trim();
    let (hour, minute) = text
        .split_once(':')
        .ok_or_else(|| VocabError(format!("提醒时间应写成 HH:MM，收到「{text}」")))?;
    let hour: u32 = hour
        .parse()
        .map_err(|_| VocabError(format!("提醒时间的小时不是数字：{text}")))?;
    let minute: u32 = minute
        .parse()
        .map_err(|_| VocabError(format!("提醒时间的分钟不是数字：{text}")))?;
    if hour > 23 || minute > 59 {
        return Err(VocabError(format!("提醒时间超出 00:00–23:59：{text}")));
    }
    Ok((format!("{hour:02}:{minute:02}"), hour, minute))
}

/// The epoch millis of `date` at `hour:minute` in the given zone; `None` uses the device zone.
pub fn local_millis(
    date: NaiveDate,
    hour: u32,
    minute: u32,
    offset_minutes: Option<i32>,
) -> Option<i64> {
    let naive = date.and_hms_opt(hour, minute, 0)?;
    if let Some(minutes) = offset_minutes {
        let offset = FixedOffset::east_opt(minutes * 60)?;
        return offset
            .from_local_datetime(&naive)
            .single()
            .map(|time| time.timestamp_millis());
    }
    Local
        .from_local_datetime(&naive)
        .single()
        .or_else(|| Local.from_local_datetime(&naive).earliest())
        .map(|time| time.timestamp_millis())
}

pub fn local_date(now_millis: i64, offset_minutes: Option<i32>) -> NaiveDate {
    if let Some(minutes) = offset_minutes {
        if let Some(offset) = FixedOffset::east_opt(minutes * 60) {
            if let Some(time) = Utc.timestamp_millis_opt(now_millis).single() {
                return time.with_timezone(&offset).date_naive();
            }
        }
    }
    Local
        .timestamp_millis_opt(now_millis)
        .single()
        .unwrap_or_else(Local::now)
        .date_naive()
}

pub fn local_today(now_millis: i64, offset_minutes: Option<i32>) -> String {
    format_date(local_date(now_millis, offset_minutes))
}

/// Useful in tests and in the page header.
pub fn weekday_cn(date: NaiveDate) -> &'static str {
    match date.weekday() {
        chrono::Weekday::Mon => "星期一",
        chrono::Weekday::Tue => "星期二",
        chrono::Weekday::Wed => "星期三",
        chrono::Weekday::Thu => "星期四",
        chrono::Weekday::Fri => "星期五",
        chrono::Weekday::Sat => "星期六",
        chrono::Weekday::Sun => "星期日",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(raw: &str) -> NaiveDate {
        parse_date(raw).unwrap()
    }

    #[test]
    fn intervals_advance_and_master() {
        assert_eq!(
            review_result(date("2026-11-13"), 0, true),
            (1, Some("2026-11-15".to_string()))
        );
        assert_eq!(
            review_result(date("2026-11-15"), 1, true),
            (2, Some("2026-11-19".to_string()))
        );
        assert_eq!(
            review_result(date("2026-11-19"), 2, true),
            (3, Some("2026-11-26".to_string()))
        );
        assert_eq!(
            review_result(date("2026-11-26"), 3, true),
            (4, Some("2026-12-11".to_string()))
        );
        assert_eq!(review_result(date("2026-12-11"), 4, true), (5, None));
    }

    #[test]
    fn wrong_returns_tomorrow() {
        assert_eq!(
            review_result(date("2026-11-13"), 3, false),
            (0, Some("2026-11-14".to_string()))
        );
    }

    #[test]
    fn reminder_time_is_normalized_and_validated() {
        assert_eq!(
            parse_reminder_time("8:05").unwrap(),
            ("08:05".to_string(), 8, 5)
        );
        assert!(parse_reminder_time("24:00").is_err());
        assert!(parse_reminder_time("20").is_err());
        assert!(parse_reminder_time("aa:bb").is_err());
    }
}
