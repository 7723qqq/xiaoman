//! Links that Xiaoman itself turns into system intents, so a skill can reach a stable system entry
//! point through the ordinary `deeplink` action (and its Sentinel check) instead of fiddly screens.
//!
//! `xiaoman://alarm/set?hour=14&minute=0&label=喝水` opens the clock's new-alarm page prefilled
//! (`ACTION_SET_ALARM`, never skipping the UI): the operator still checks the page and taps save.
//! Every other URI stays an ordinary `ACTION_VIEW` deeplink and never reaches this module.
//!
//! The result is the intent's payload, not an `Intent`: the Java side builds
//! `Intent(link.action()).putExtra(EXTRA_HOUR, hour).putExtra(EXTRA_MINUTES, minute)
//! .putExtra(EXTRA_SKIP_UI, false)` and, when a label is present,
//! `.putExtra(EXTRA_MESSAGE, label)`.

use std::error::Error;
use std::fmt;

/// URI scheme of Xiaoman's own links.
pub const SCHEME: &str = "xiaoman";

/// Longest alarm label kept, in characters; the rest is dropped.
pub const MAX_LABEL: usize = 60;

/// `android.provider.AlarmClock.ACTION_SET_ALARM`.
pub const ACTION_SET_ALARM: &str = "android.intent.action.SET_ALARM";

/// `android.provider.AlarmClock.EXTRA_HOUR`, an int.
pub const EXTRA_HOUR: &str = "android.intent.extra.alarm.HOUR";

/// `android.provider.AlarmClock.EXTRA_MINUTES`, an int.
pub const EXTRA_MINUTES: &str = "android.intent.extra.alarm.MINUTES";

/// `android.provider.AlarmClock.EXTRA_MESSAGE`, a string.
pub const EXTRA_MESSAGE: &str = "android.intent.extra.alarm.MESSAGE";

/// `android.provider.AlarmClock.EXTRA_SKIP_UI`, a boolean.
pub const EXTRA_SKIP_UI: &str = "android.intent.extra.alarm.SKIP_UI";

/// The prefill of a new alarm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlarmLink {
    pub hour: i32,
    pub minute: i32,
    pub label: Option<String>,
    /// Always false: the clock UI is never skipped, the operator checks the page and taps save.
    pub skip_ui: bool,
}

/// A system intent a Xiaoman link maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppLink {
    SetAlarm(AlarmLink),
}

impl AppLink {
    /// The Android action of the intent to start.
    pub fn action(&self) -> &'static str {
        match self {
            Self::SetAlarm(_) => ACTION_SET_ALARM,
        }
    }

    pub fn alarm(&self) -> Option<&AlarmLink> {
        match self {
            Self::SetAlarm(link) => Some(link),
        }
    }
}

/// Why a Xiaoman link was refused; the messages are the ones the user sees, byte for byte as in
/// the Kotlin original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppLinkError {
    /// A Xiaoman link whose host and path name no supported entry point.
    Unsupported { uri: String },
    /// `hour` missing or outside 0..=23.
    BadHour,
    /// `minute` outside 0..=59.
    BadMinute,
}

impl fmt::Display for AppLinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { uri } => {
                write!(
                    f,
                    "不支持的小满链接：{uri}（目前只支持 xiaoman://alarm/set）"
                )
            }
            Self::BadHour => f.write_str("xiaoman://alarm/set 需要 hour=0..23"),
            Self::BadMinute => f.write_str("xiaoman://alarm/set 的 minute 需要 0..59"),
        }
    }
}

impl Error for AppLinkError {}

/// The intent for `uri`, or `None` when it is not a Xiaoman link.
///
/// `Err` for a malformed Xiaoman link: an unknown entry point, a missing or out-of-range `hour`, an
/// out-of-range `minute`.
pub fn link_for(uri: &str) -> Result<Option<AppLink>, AppLinkError> {
    let Some((scheme, rest)) = uri.split_once(':') else {
        return Ok(None);
    };
    if !scheme.eq_ignore_ascii_case(SCHEME) {
        return Ok(None);
    }
    let rest = rest.strip_prefix("//").unwrap_or_default();
    let rest = rest.split('#').next().unwrap_or_default();
    let (authority, tail) = match rest.find(['/', '?']) {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    let (path, query) = match tail.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (tail, None),
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host.split_once(':').map_or(host, |(host, _port)| host);
    match format!("{host}{path}").trim_end_matches('/') {
        "alarm/set" => set_alarm(query).map(|link| Some(AppLink::SetAlarm(link))),
        _ => Err(AppLinkError::Unsupported {
            uri: uri.to_string(),
        }),
    }
}

fn set_alarm(query: Option<&str>) -> Result<AlarmLink, AppLinkError> {
    let hour = query_param(query, "hour")
        .map(|value| value.trim().to_string())
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|hour| (0..=23).contains(hour))
        .ok_or(AppLinkError::BadHour)?;
    let minute = match query_param(query, "minute").map(|value| value.trim().to_string()) {
        Some(value) if !value.is_empty() => value,
        _ => "0".to_string(),
    };
    let minute = minute
        .parse::<i32>()
        .ok()
        .filter(|minute| (0..=59).contains(minute))
        .ok_or(AppLinkError::BadMinute)?;
    let label = query_param(query, "label")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(MAX_LABEL).collect());
    Ok(AlarmLink {
        hour,
        minute,
        label,
        skip_ui: false,
    })
}

/// The first value of `key`, decoded like Android's `Uri.getQueryParameter`: `%xx` escapes become
/// bytes of UTF-8 and `+` becomes a space. Parameters without `=` have no value.
fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    for pair in query?.split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        if decode_query(raw_key) == key {
            return Some(decode_query(raw_value));
        }
    }
    None
}

fn decode_query(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                match (hex_value(bytes[index + 1]), hex_value(bytes[index + 2])) {
                    (Some(high), Some(low)) => {
                        out.push(high << 4 | low);
                        index += 3;
                    }
                    _ => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
