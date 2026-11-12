//! Parsing of `Xiaoman: <run_id> <EventKind> <json>` logcat lines.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::traces::{event_fields, truthy};

// threadtime: "10-31 12:00:00.123  1234  1250 I Xiaoman: r1 StepPlanned {...}"
// brief:      "I/Xiaoman( 1234): r1 StepPlanned {...}"
static TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[\s/])Xiaoman\s*(?:\(\s*\d+\))?\s*:\s(?P<msg>.*)$").unwrap()
});
static TIME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\d\d-\d\d\s+)?(?P<time>\d\d:\d\d:\d\d\.\d+)").unwrap());
static KIND_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z][A-Za-z0-9]*$").unwrap());

/// Kinds that end a run.
pub const TERMINAL_KINDS: [&str; 2] = ["PhoneTaskFinished", "Failed"];

/// One parsed Xiaoman event line.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEvent {
    pub run_id: String,
    pub kind: String,
    /// The JSON object after the kind, when it parses.
    pub data: Option<Value>,
    /// Raw text after the kind.
    pub payload: String,
    /// `HH:MM:SS.mmm` from logcat, if present.
    pub time: Option<String>,
}

impl LogEvent {
    pub fn terminal(&self) -> bool {
        TERMINAL_KINDS.contains(&self.kind.as_str())
    }

    /// For terminal events: `Some(true)`/`Some(false)`, or `None` when unknown.
    pub fn success(&self) -> Option<bool> {
        if self.kind == "Failed" {
            return Some(false);
        }
        let data = self.data.as_ref()?;
        if let Some(success) = data.get("success") {
            return Some(truthy(success));
        }
        match data.get("ok") {
            Some(ok) if !ok.is_null() => Some(truthy(ok)),
            _ => None,
        }
    }
}

/// The log message of a Xiaoman-tagged logcat line, or `None`.
pub fn message_of(line: &str) -> Option<&str> {
    let line = line.trim_end_matches(['\r', '\n']);
    TAG_RE
        .captures(line)
        .map(|captures| captures.name("msg").unwrap().as_str())
}

/// Parses the message part into an event.
pub fn parse_message(message: &str, time: Option<&str>) -> Option<LogEvent> {
    let parts = split_fields(message);
    if parts.len() < 2 || !KIND_RE.is_match(&parts[1]) {
        return None;
    }
    let payload = parts.get(2).cloned().unwrap_or_default();
    let data = if payload.is_empty() {
        None
    } else {
        serde_json::from_str::<Value>(&payload)
            .ok()
            .filter(Value::is_object)
    };
    Some(LogEvent {
        run_id: parts[0].clone(),
        kind: parts[1].clone(),
        data,
        payload,
        time: time.map(str::to_string),
    })
}

/// Parses one logcat line into an event, or `None` if it is not an event line.
///
/// Accepts threadtime/brief formatted lines and bare `-v raw` messages.
pub fn parse_line(line: &str) -> Option<LogEvent> {
    let line = line.trim_end_matches(['\r', '\n']);
    let mut time = None;
    let message = match message_of(line) {
        Some(message) => {
            if let Some(captures) = TIME_RE.captures(line) {
                let text = captures
                    .name("time")
                    .map(|capture| capture.as_str())
                    .unwrap_or_default();
                time = Some(text[..text.len().min(12)].to_string());
            }
            message
        }
        None => {
            if line.starts_with("--------- ") {
                return None;
            }
            line
        }
    };
    parse_message(message, time.as_deref())
}

/// `<time> <run_id> <kind>  <fields>` for display.
pub fn format_event(event: &LogEvent) -> String {
    let head = format!(
        "{} {} {}",
        event.time.as_deref().unwrap_or(""),
        event.run_id,
        event.kind
    );
    let tail = match &event.data {
        None => event.payload.clone(),
        Some(data) => {
            let fields = event_fields(data);
            let mut bits = Vec::new();
            if !fields.step.is_empty() {
                bits.push(format!("#{}", fields.step));
            }
            if !fields.action.is_empty() {
                bits.push(fields.action);
            }
            if !fields.decision.is_empty() {
                bits.push(format!("[{}]", fields.decision));
            }
            if !fields.ok.is_empty() {
                bits.push(format!("ok={}", fields.ok));
            }
            if !fields.text.is_empty() {
                bits.push(format!("| {}", fields.text));
            }
            bits.join(" ")
        }
    };
    if tail.is_empty() {
        head.trim().to_string()
    } else {
        format!("{head}  {tail}").trim().to_string()
    }
}

/// Splits `message` into at most three whitespace-separated fields, keeping the
/// remainder's inner whitespace, like Python's `split(None, 2)`.
fn split_fields(message: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = message.trim();
    for _ in 0..2 {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        parts.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    let remainder = rest.trim();
    if !remainder.is_empty() {
        parts.push(remainder.to_string());
    }
    parts
}
