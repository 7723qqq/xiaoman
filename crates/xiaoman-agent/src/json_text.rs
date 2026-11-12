//! Small helpers for model-produced JSON.

use serde_json::{Map, Value};

use crate::operator_reply_parser::{extract_first_object, strip_fences};

pub struct JsonText;

impl JsonText {
    /// The first `{...}` object in `text` (code fences and surrounding prose allowed), or `None`.
    pub fn first_object(text: &str) -> Option<Map<String, Value>> {
        let raw = extract_first_object(&strip_fences(text))?;
        match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Object(map)) => Some(map),
            _ => None,
        }
    }

    pub fn string(object: &Map<String, Value>, key: &str) -> Option<String> {
        let value = object.get(key)?;
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            Value::Bool(flag) => flag.to_string(),
            _ => return None,
        };
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    pub fn bool_of(object: &Map<String, Value>, key: &str) -> Option<bool> {
        let value = object.get(key)?;
        if let Some(flag) = value.as_bool() {
            return Some(flag);
        }
        let text = value.as_str()?.trim().to_lowercase();
        match text.as_str() {
            "yes" | "1" | "是" => Some(true),
            "no" | "0" | "否" => Some(false),
            _ => None,
        }
    }

    pub fn string_list(object: &Map<String, Value>, key: &str) -> Vec<String> {
        match object.get(key) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| match item {
                    Value::String(text) => Some(text.trim().to_string()),
                    Value::Number(number) => Some(number.to_string()),
                    _ => None,
                })
                .filter(|item| !item.is_empty())
                .collect(),
            Some(Value::String(text)) => text
                .split([',', '，', ' '])
                .map(|part| part.trim())
                .map(|part| part.strip_prefix('#').unwrap_or(part))
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// JSON string literal of `text`.
    pub fn quote(text: &str) -> String {
        serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
    }
}
