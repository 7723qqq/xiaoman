//! Tolerant parser for operator model replies.
//!
//! Strips code fences, takes the first balanced `{...}`, accepts `{"action": {...}}` or a bare
//! action, common type synonyms, field aliases, and points as `[x, y]`, `{x, y}` or `"x,y"`
//! (clamped to 0..1000).

use serde_json::{Map, Value};
use xiaoman_api::{Action, Point};

/// Result of [`parse`].
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedReply {
    Ok {
        action: Action,
        thought: Option<String>,
    },
    Error {
        message: String,
    },
}

pub fn parse(reply: &str) -> ParsedReply {
    let Some(raw) = extract_first_object(&strip_fences(reply)) else {
        return ParsedReply::Error {
            message: "回复中没有 JSON 对象".to_string(),
        };
    };
    let root = match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => map,
        _ => {
            return ParsedReply::Error {
                message: "JSON 格式错误".to_string(),
            };
        }
    };
    let thought = string_field(&root, &["thought", "thinking", "reasoning"]);
    let action_object: Map<String, Value> = match root.get("action") {
        Some(Value::Object(nested)) => nested.clone(),
        // {"action": "tap", "nodeId": 3}
        Some(Value::String(kind)) => {
            let mut merged = root.clone();
            merged.insert("type".into(), Value::String(kind.clone()));
            merged
        }
        _ => root,
    };
    match to_action(&action_object) {
        Ok(action) => ParsedReply::Ok { action, thought },
        Err(message) => ParsedReply::Error { message },
    }
}

pub fn strip_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("```") {
        out.push_str(&rest[..index]);
        rest = &rest[index + 3..];
        let letters = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        rest = &rest[letters..];
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// The first balanced JSON object in `text`, honoring strings and escapes.
pub fn extract_first_object(text: &str) -> Option<String> {
    let mut search = 0;
    while let Some(offset) = text[search..].find('{') {
        let start = search + offset;
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escaped = false;
        for (index, c) in text[start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
                continue;
            }
            match c {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(text[start..start + index + 1].to_string());
                    }
                }
                _ => {}
            }
        }
        search = start + 1;
    }
    None
}

fn synonym(normalized: &str) -> Option<&'static str> {
    let kind = match normalized {
        "tap" | "click" | "press" | "touch" | "tap_node" | "click_node" => "tap",
        "long_press" | "longpress" | "long_click" | "longclick" | "hold" => "long_press",
        "swipe" | "scroll" | "drag" | "fling" => "swipe",
        "type" | "input" | "input_text" | "type_text" | "text" | "enter_text" | "set_text" => {
            "type"
        }
        "back" | "go_back" | "press_back" | "navigate_back" => "back",
        "home" | "go_home" | "press_home" => "home",
        "launch" | "launch_app" | "open_app" | "start_app" | "open" => "launch",
        "deeplink" | "deep_link" | "open_deeplink" | "open_url" | "open_uri" | "url" => "deeplink",
        "wait" | "sleep" | "pause" => "wait",
        "ask_user" | "ask" | "ask_human" | "question" | "takeover" | "take_over" | "handover" => {
            "ask_user"
        }
        "finish" | "done" | "complete" | "completed" | "stop" | "end" | "fail" | "failed" => {
            "finish"
        }
        _ => return None,
    };
    Some(kind)
}

fn to_action(object: &Map<String, Value>) -> Result<Action, String> {
    let raw_type = string_field(object, &["type", "action_type", "actionType", "name", "op"])
        .ok_or_else(|| "缺少 action.type".to_string())?;
    let normalized = raw_type.trim().to_lowercase().replace(['-', ' '], "_");
    let kind = synonym(&normalized).ok_or_else(|| format!("未知的操作类型 \"{raw_type}\""))?;
    let target = string_field(object, &["target", "element", "label"]);
    let reason = string_field(object, &["reason", "why"]);

    match kind {
        "tap" => {
            let node_id = node_id(object);
            let point = point_field(object);
            if node_id.is_none() && point.is_none() {
                return Err("tap 需要 nodeId 或 point".to_string());
            }
            Ok(Action::Tap {
                point,
                node_id,
                target,
                reason,
            })
        }
        "long_press" => {
            let node_id = node_id(object);
            let point = point_field(object);
            if node_id.is_none() && point.is_none() {
                return Err("long_press 需要 nodeId 或 point".to_string());
            }
            let duration_ms =
                long_field(object, &["durationMs", "duration_ms", "duration"]).unwrap_or(800);
            Ok(Action::LongPress {
                point,
                node_id,
                duration_ms,
                target,
                reason,
            })
        }
        "swipe" => swipe(object, target, reason),
        "type" => {
            let text = string_field(object, &["text", "content", "value", "input"])
                .ok_or_else(|| "type 需要 text".to_string())?;
            let submit = bool_field(object, &["submit", "enter"]).unwrap_or(false);
            Ok(Action::TypeText {
                text,
                node_id: node_id(object),
                submit,
                target,
                reason,
            })
        }
        "back" => Ok(Action::Back { target, reason }),
        "home" => Ok(Action::Home { target, reason }),
        "launch" => {
            let package_name = string_field(
                object,
                &[
                    "packageName",
                    "package_name",
                    "package",
                    "app",
                    "appPackage",
                ],
            )
            .ok_or_else(|| "launch 需要 packageName".to_string())?;
            Ok(Action::LaunchApp {
                package_name,
                target,
                reason,
            })
        }
        "deeplink" => {
            let uri = string_field(object, &["uri", "url", "link", "deeplink"])
                .ok_or_else(|| "deeplink 需要 uri".to_string())?;
            let package_name = string_field(object, &["packageName", "package_name", "package"]);
            Ok(Action::OpenDeeplink {
                uri,
                package_name,
                target,
                reason,
            })
        }
        "wait" => {
            let millis = long_field(
                object,
                &["millis", "ms", "durationMs", "duration_ms", "duration"],
            )
            .or_else(|| {
                double_field(object, &["seconds", "secs"])
                    .map(|seconds| (seconds * 1000.0).round() as i64)
            })
            .unwrap_or(1000);
            Ok(Action::Wait {
                millis: millis.clamp(0, 60_000),
                target,
                reason,
            })
        }
        "ask_user" => {
            let question = string_field(object, &["question", "text", "message", "prompt"])
                .ok_or_else(|| "ask_user 需要 question".to_string())?;
            let takeover = bool_field(object, &["takeover", "take_over"])
                .unwrap_or_else(|| normalized.starts_with("take") || normalized == "handover");
            Ok(Action::AskUser {
                question,
                takeover,
                target,
                reason,
            })
        }
        "finish" => {
            let status = string_field(object, &["status"]).map(|status| status.to_lowercase());
            let success = bool_field(object, &["success", "ok"]).unwrap_or_else(|| match &status {
                Some(status) => matches!(
                    status.as_str(),
                    "success" | "succeeded" | "ok" | "done" | "completed"
                ),
                None => !normalized.starts_with("fail"),
            });
            let summary = string_field(object, &["summary", "message", "result", "text", "answer"])
                .unwrap_or_else(|| {
                    if success {
                        "完成".to_string()
                    } else {
                        "未完成".to_string()
                    }
                });
            Ok(Action::Finish {
                success,
                summary,
                target,
                reason,
            })
        }
        _ => Err(format!("未知的操作类型 \"{raw_type}\"")),
    }
}

fn swipe(
    object: &Map<String, Value>,
    target: Option<String>,
    reason: Option<String>,
) -> Result<Action, String> {
    let duration_ms = long_field(object, &["durationMs", "duration_ms", "duration"]).unwrap_or(300);
    let from = point_at(object, &["from", "start", "from_point"]);
    let to = point_at(object, &["to", "end", "to_point"]);
    if let (Some(from), Some(to)) = (from, to) {
        return Ok(Action::Swipe {
            from,
            to,
            duration_ms,
            target,
            reason,
        });
    }
    // {"type": "scroll", "direction": "up"}: finger direction.
    let (from, to) = match string_field(object, &["direction", "dir"])
        .map(|direction| direction.to_lowercase())
        .as_deref()
    {
        Some("up") => (point(500, 750), point(500, 250)),
        Some("down") => (point(500, 250), point(500, 750)),
        Some("left") => (point(800, 500), point(200, 500)),
        Some("right") => (point(200, 500), point(800, 500)),
        _ => return Err("swipe 需要 from/to 或 direction".to_string()),
    };
    Ok(Action::Swipe {
        from,
        to,
        duration_ms,
        target,
        reason,
    })
}

fn point(x: i32, y: i32) -> Point {
    Point::new(x, y).expect("in range")
}

fn first<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .find(|value| !value.is_null())
}

fn primitive_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn string_field(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    let text = primitive_text(first(object, keys)?)?;
    (!text.trim().is_empty()).then_some(text)
}

fn bool_field(object: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    let value = first(object, keys)?;
    if let Some(flag) = value.as_bool() {
        return Some(flag);
    }
    let text = primitive_text(value)?;
    match text.trim().to_lowercase().as_str() {
        "yes" | "1" | "true" => Some(true),
        "no" | "0" | "false" => Some(false),
        _ => None,
    }
}

fn double_field(object: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    let value = first(object, keys)?;
    if let Some(number) = value.as_f64() {
        return Some(number);
    }
    primitive_text(value)?.trim().parse::<f64>().ok()
}

fn long_field(object: &Map<String, Value>, keys: &[&str]) -> Option<i64> {
    let value = first(object, keys)?;
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    double_field(object, keys).map(|number| number.round() as i64)
}

fn node_id(object: &Map<String, Value>) -> Option<i32> {
    let value = first(
        object,
        &[
            "nodeId",
            "node_id",
            "nodeID",
            "id",
            "node",
            "element_id",
            "elementId",
            "index",
        ],
    )?;
    let text = primitive_text(value)?;
    let text = text.trim().trim_start_matches('[').trim_end_matches(']');
    if let Ok(number) = text.parse::<i32>() {
        return Some(number);
    }
    text.parse::<f64>().ok().map(|number| number as i32)
}

fn point_at(object: &Map<String, Value>, keys: &[&str]) -> Option<Point> {
    first(object, keys).and_then(to_point)
}

fn point_field(object: &Map<String, Value>) -> Option<Point> {
    if let Some(found) = point_at(
        object,
        &[
            "point",
            "coordinate",
            "coordinates",
            "position",
            "pos",
            "xy",
        ],
    ) {
        return Some(found);
    }
    let x = double_field(object, &["x"])?;
    let y = double_field(object, &["y"])?;
    Some(clamp_point(x, y))
}

fn to_point(value: &Value) -> Option<Point> {
    match value {
        Value::Array(items) if items.len() >= 2 => {
            let x = primitive_text(&items[0])?.trim().parse::<f64>().ok()?;
            let y = primitive_text(&items[1])?.trim().parse::<f64>().ok()?;
            Some(clamp_point(x, y))
        }
        Value::Object(map) => {
            let x = double_field(map, &["x"])?;
            let y = double_field(map, &["y"])?;
            Some(clamp_point(x, y))
        }
        Value::String(text) => {
            let numbers: Vec<f64> = number_runs(text)
                .into_iter()
                .filter_map(|run| run.parse::<f64>().ok())
                .collect();
            (numbers.len() >= 2).then(|| clamp_point(numbers[0], numbers[1]))
        }
        _ => None,
    }
}

/// `-?\d+(\.\d+)?` occurrences, in order.
fn number_runs(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let start = if (chars[index] == '-'
            && index + 1 < chars.len()
            && chars[index + 1].is_ascii_digit())
            || chars[index].is_ascii_digit()
        {
            index
        } else {
            index += 1;
            continue;
        };
        let mut end = start + 1;
        while end < chars.len() && chars[end].is_ascii_digit() {
            end += 1;
        }
        if end + 1 < chars.len() && chars[end] == '.' && chars[end + 1].is_ascii_digit() {
            end += 1;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
        }
        out.push(chars[start..end].iter().collect());
        index = end;
    }
    out
}

fn clamp_point(x: f64, y: f64) -> Point {
    let clamp = |value: f64| value.round().clamp(0.0, Point::NORM as f64) as i32;
    Point::new(clamp(x), clamp(y)).expect("clamped point")
}
