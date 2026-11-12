//! Reading `trace.jsonl` files and rendering events as readable text.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Which timezone timestamps are shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Local,
    Utc,
}

// --------------------------------------------------------------------------- text width helpers

/// Width of `s` in terminal cells: East Asian wide characters count 2, combining marks 0.
pub fn display_width(s: &str) -> usize {
    s.chars()
        .filter(|ch| !is_combining(*ch))
        .map(char_width)
        .sum()
}

/// Shortens `s` to `width` display cells, ending with an ellipsis.
pub fn truncate(s: &str, width: usize) -> String {
    if width == 0 || display_width(s) <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = char_width(ch);
        if used + w > width - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Pads `s` on the right to `width` display cells.
pub fn pad(s: &str, width: usize) -> String {
    let mut out = s.to_string();
    out.push_str(&" ".repeat(width.saturating_sub(display_width(s))));
    out
}

fn char_width(ch: char) -> usize {
    if is_wide(ch) { 2 } else { 1 }
}

fn is_combining(ch: char) -> bool {
    matches!(ch as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x05BF..=0x05BF
        | 0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x0610..=0x061A | 0x064B..=0x065F
        | 0x0670..=0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8
        | 0x06EA..=0x06ED | 0x0900..=0x0903 | 0x093A..=0x093C | 0x093E..=0x094F
        | 0x0951..=0x0957 | 0x0962..=0x0963 | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF
        | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x2FFFD
        | 0x30000..=0x3FFFD)
}

// --------------------------------------------------------------------------- summaries

/// Python truthiness for JSON values.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn py_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => {
            serde_json::to_string(text).unwrap_or_else(|_| format!("\"{text}\""))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(py_json).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Object(map) => {
            let entries: Vec<String> = map
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{}: {}",
                        serde_json::to_string(key).unwrap_or_default(),
                        py_json(value)
                    )
                })
                .collect();
            format!("{{{}}}", entries.join(", "))
        }
    }
}

fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(value) => (if *value { "True" } else { "False" }).to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        other => py_json(other),
    }
}

/// A value on one line: strings are whitespace-collapsed, other values are JSON.
pub fn one_line(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => collapse(text),
        other => collapse(&py_json(other)),
    }
}

fn quote(value: Option<&Value>) -> String {
    format!("\"{}\"", value.map(one_line).unwrap_or_default())
}

fn point(value: Option<&Value>) -> String {
    match value {
        Some(Value::Object(map)) if map.contains_key("x") && map.contains_key("y") => {
            format!("({},{})", py_str(&map["x"]), py_str(&map["y"]))
        }
        Some(Value::Array(items)) if items.len() == 2 => {
            format!("({},{})", py_str(&items[0]), py_str(&items[1]))
        }
        Some(value) => one_line(value),
        None => String::new(),
    }
}

/// Short human form of an Action JSON object (`type` is the discriminator).
pub fn summarize_action(action: Option<&Value>) -> String {
    let Some(action) = action else {
        return String::new();
    };
    let Value::Object(map) = action else {
        return one_line(action);
    };
    let mut parts = vec![
        map.get("type")
            .map(py_str)
            .unwrap_or_else(|| "?".to_string()),
    ];
    let where_ = match map.get("nodeId").filter(|value| !value.is_null()) {
        Some(node_id) => Some(format!("#{}", py_str(node_id))),
        None => map
            .get("point")
            .filter(|value| !value.is_null())
            .map(|value| point(Some(value))),
    };
    match map.get("type").and_then(Value::as_str).unwrap_or_default() {
        "tap" | "long_press" => {
            if let Some(where_) = &where_ {
                parts.push(where_.clone());
            }
            if let Some(target) = map.get("target").filter(|value| truthy(value)) {
                parts.push(quote(Some(target)));
            }
        }
        "type" => {
            parts.push(quote(map.get("text")));
            if let Some(target) = map.get("target").filter(|value| truthy(value)) {
                parts.push(format!("→ {}", quote(Some(target))));
            } else if let Some(where_) = &where_ {
                parts.push(format!("→ {where_}"));
            }
            if map.get("submit").is_some_and(truthy) {
                parts.push("+submit".to_string());
            }
        }
        "swipe" => {
            parts.push(format!(
                "{}→{}",
                point(map.get("from")),
                point(map.get("to"))
            ));
            if let Some(target) = map.get("target").filter(|value| truthy(value)) {
                parts.push(quote(Some(target)));
            }
        }
        "launch" => {
            let package = ["packageName", "package"]
                .iter()
                .find_map(|key| map.get(*key).filter(|value| truthy(value)));
            parts.push(package.map(py_str).unwrap_or_default());
        }
        "deeplink" => parts.push(map.get("uri").map(py_str).unwrap_or_default()),
        "wait" => parts.push(format!(
            "{}ms",
            map.get("millis").map(py_str).unwrap_or_default()
        )),
        "ask_user" => {
            parts.push(quote(map.get("question")));
            if map.get("takeover").is_some_and(truthy) {
                parts.push("(takeover)".to_string());
            }
        }
        "finish" => {
            parts.push(
                (if map.get("success").is_some_and(truthy) {
                    "ok"
                } else {
                    "fail"
                })
                .to_string(),
            );
            if let Some(summary) = map.get("summary").filter(|value| truthy(value)) {
                parts.push(quote(Some(summary)));
            }
        }
        _ => {
            if let Some(target) = map.get("target").filter(|value| truthy(value)) {
                parts.push(quote(Some(target)));
            }
        }
    }
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn decision_of(value: Option<&Value>) -> String {
    match value {
        None => String::new(),
        Some(Value::Object(map)) => {
            let verdict = ["verdict", "type", "kind"]
                .iter()
                .find_map(|key| map.get(*key).filter(|value| truthy(value)));
            let reason = map.get("reason").filter(|value| truthy(value));
            match reason {
                Some(reason) => {
                    let verdict = verdict.map(py_str).unwrap_or_default();
                    format!("{verdict}: {}", py_str(reason))
                        .trim_matches(|ch| ch == ':' || ch == ' ')
                        .to_string()
                }
                None => verdict.map(py_str).unwrap_or_default(),
            }
        }
        Some(value) => one_line(value),
    }
}

/// Display fields normalized from a trace line or a logged AgentEvent JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventFields {
    pub step: String,
    pub action: String,
    pub decision: String,
    pub ok: String,
    pub text: String,
}

pub fn event_fields(data: &Value) -> EventFields {
    let Some(map) = data.as_object() else {
        return EventFields::default();
    };
    let step = if map.contains_key("step") {
        map.get("step")
    } else {
        map.get("stepIndex")
    };

    let mut ok = map.get("ok").filter(|value| !value.is_null());
    if ok.is_none() {
        let from_result = map
            .get("result")
            .and_then(Value::as_object)
            .and_then(|result| result.get("ok"));
        ok = from_result.or_else(|| map.get("success"));
    }

    let mut text = map.get("text").filter(|value| !value.is_null());
    if text.is_none() {
        for key in [
            "summary",
            "message",
            "question",
            "goal",
            "skillName",
            "permalink",
        ] {
            if let Some(value) = map.get(key).filter(|value| !value.is_null()) {
                text = Some(value);
                break;
            }
        }
    }
    if text.is_none() {
        text = map
            .get("result")
            .and_then(Value::as_object)
            .and_then(|result| result.get("message"));
    }

    EventFields {
        step: match step {
            Some(value) if !value.is_null() => py_str(value),
            _ => String::new(),
        },
        action: summarize_action(map.get("action")),
        decision: decision_of(map.get("decision")),
        ok: match ok {
            Some(value) if !value.is_null() => {
                (if truthy(value) { "yes" } else { "no" }).to_string()
            }
            _ => String::new(),
        },
        text: text.map(one_line).unwrap_or_default(),
    }
}

/// `HH:MM:SS.mmm` for a millisecond timestamp, or `""` when it is not a number.
pub fn format_time(value: Option<&Value>, zone: Zone) -> String {
    let Some(Value::Number(number)) = value else {
        return String::new();
    };
    let millis = match number.as_i64() {
        Some(millis) => millis,
        None => match number.as_f64() {
            Some(value) if value.is_finite() => value.trunc() as i64,
            _ => return String::new(),
        },
    };
    let Some(datetime) = chrono::DateTime::from_timestamp_millis(millis) else {
        return String::new();
    };
    match zone {
        Zone::Utc => datetime.format("%H:%M:%S%.3f").to_string(),
        Zone::Local => datetime
            .with_timezone(&chrono::Local)
            .format("%H:%M:%S%.3f")
            .to_string(),
    }
}

// --------------------------------------------------------------------------- trace files

/// Reads a trace.jsonl; invalid lines become `{"kind": "<invalid>", "text": raw, "_line": n}`.
pub fn load_trace(path: &Path) -> io::Result<Vec<Value>> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut events = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let mut data = match serde_json::from_str::<Value>(raw) {
            Ok(value @ Value::Object(_)) => value,
            _ => serde_json::json!({"kind": "<invalid>", "text": raw}),
        };
        if let Value::Object(map) = &mut data {
            map.insert("_line".to_string(), serde_json::json!(index + 1));
        }
        events.push(data);
    }
    Ok(events)
}

/// Maps a `traces show` argument to a trace.jsonl path, or `None` if it is not local.
pub fn resolve_trace(arg: &str, local_root: &Path) -> Option<PathBuf> {
    let path = Path::new(arg);
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if path.is_dir() {
        let candidate = path.join("trace.jsonl");
        return candidate.is_file().then_some(candidate);
    }
    let candidate = local_root.join(arg).join("trace.jsonl");
    candidate.is_file().then_some(candidate)
}

const COLUMNS: [&str; 7] = ["time", "step", "kind", "action", "decision", "ok", "text"];
const MAX_WIDTH: [(usize, usize); 3] = [(3, 44), (4, 32), (6, 60)];

/// Renders events as a step table.
pub fn render_table(events: &[Value], zone: Zone, full: bool) -> String {
    let mut rows: Vec<Vec<String>> = events
        .iter()
        .map(|event| {
            let fields = event_fields(event);
            let object = event.as_object();
            vec![
                format_time(object.and_then(|map| map.get("time")), zone),
                fields.step,
                object
                    .and_then(|map| map.get("kind"))
                    .map(py_str)
                    .unwrap_or_default(),
                fields.action,
                fields.decision,
                fields.ok,
                fields.text,
            ]
        })
        .collect();
    if !full {
        for row in &mut rows {
            for (index, width) in MAX_WIDTH {
                row[index] = truncate(&row[index], width);
            }
        }
    }
    let mut widths: Vec<usize> = COLUMNS.iter().map(|column| display_width(column)).collect();
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(display_width(cell));
        }
    }
    let mut lines = Vec::new();
    let header: Vec<String> = COLUMNS.iter().map(|column| column.to_uppercase()).collect();
    emit(&mut lines, &header, &widths);
    let separator: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    emit(&mut lines, &separator, &widths);
    for row in &rows {
        emit(&mut lines, row, &widths);
    }
    lines.join("\n")
}

fn emit(lines: &mut Vec<String>, cells: &[String], widths: &[usize]) {
    let mut line = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            line.push_str("  ");
        }
        line.push_str(&pad(cell, widths[index]));
    }
    lines.push(line.trim_end().to_string());
}

/// One-line outcome summary plus screenshot count.
pub fn trace_footer(events: &[Value], trace_dir: Option<&Path>) -> String {
    let mut parts = vec![format!("{} events", events.len())];
    for event in events.iter().rev() {
        match event.get("kind").and_then(Value::as_str) {
            Some("PhoneTaskFinished") => {
                let fields = event_fields(event);
                parts.push(format!(
                    "finished: {}",
                    if fields.ok == "yes" {
                        "success"
                    } else {
                        "failure"
                    }
                ));
                break;
            }
            Some("Failed") => {
                parts.push("failed".to_string());
                break;
            }
            _ => {}
        }
    }
    if let Some(dir) = trace_dir {
        if dir.is_dir() {
            let shots = std::fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .filter(|entry| {
                            let name = entry.file_name();
                            let name = name.to_string_lossy();
                            name.starts_with("step-") && name.ends_with(".png")
                        })
                        .count()
                })
                .unwrap_or(0);
            if shots > 0 {
                parts.push(format!("{shots} screenshots in {}", dir.display()));
            }
        }
    }
    parts.join(", ")
}
