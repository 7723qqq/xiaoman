//! Memory kept in the student's browser (`localStorage`), not on the server.
//!
//! Each chat request carries `memory: {notes: [{permalink, markdown, updatedAt}]}`; the markdown is
//! the same Basic Memory note format the Android app writes to files. The server loads it into the
//! session's [`crate::InMemoryMemory`] for that turn and reports every change back as a `memory`
//! event so the page can save it: `{type: "memory", action: "saved", permalink, title, markdown,
//! updatedAt}` or `{type: "memory", action: "deleted", permalink}`. Nothing is written to disk.

use serde_json::{Value, json};
use xiaoman_api::Note;
use xiaoman_memory::{BasicMemoryFormat, normalize};

pub const MAX_NOTES: usize = 200;
pub const MAX_MARKDOWN_CHARS: usize = 8_000;
pub const MAX_PERMALINK_CHARS: usize = 120;

/// Marker type for the browser-memory helpers; they are free functions, as in the Kotlin version.
pub struct BrowserMemory;

/// Notes accepted from the request, and how many were dropped for shape or size.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadedNotes {
    pub notes: Vec<Note>,
    pub skipped: usize,
}

impl BrowserMemory {
    /// `None` when the request has no `memory` field (older pages): the session keeps its own memory.
    pub fn parse(body: &Value) -> Option<LoadedNotes> {
        let memory = body.get("memory")?.as_object()?;
        let items = memory.get("notes").and_then(Value::as_array);
        let items = items.map(Vec::as_slice).unwrap_or(&[]);
        let mut skipped = items.len().saturating_sub(MAX_NOTES);
        let mut notes: Vec<Note> = Vec::new();
        for item in items.iter().take(MAX_NOTES) {
            match note(item) {
                Some(note) => match notes
                    .iter_mut()
                    .find(|existing| existing.permalink == note.permalink)
                {
                    Some(existing) => *existing = note,
                    None => notes.push(note),
                },
                None => skipped += 1,
            }
        }
        Some(LoadedNotes { notes, skipped })
    }

    pub fn saved(note: &Note) -> Value {
        json!({
            "type": "memory",
            "action": "saved",
            "permalink": note.permalink,
            "title": note.title,
            "markdown": BasicMemoryFormat::render(note),
            "updatedAt": note.updated_at_millis,
        })
    }

    pub fn deleted(permalink: &str) -> Value {
        json!({ "type": "memory", "action": "deleted", "permalink": permalink })
    }
}

/// A page-supplied markdown note parsed into the shared note shape.
fn note(item: &Value) -> Option<Note> {
    let object = item.as_object()?;
    let permalink = object
        .get("permalink")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let markdown = object
        .get("markdown")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if permalink.is_empty()
        || permalink.chars().count() > MAX_PERMALINK_CHARS
        || markdown.trim().is_empty()
        || markdown.chars().count() > MAX_MARKDOWN_CHARS
    {
        return None;
    }
    let permalink = normalize(permalink).ok()?;
    let updated_at = object.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
    Some(BasicMemoryFormat::parse(markdown, &permalink, updated_at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xiaoman_api::Observation;

    fn coffee() -> Value {
        json!({
            "permalink": "偏好/咖啡",
            "markdown": "---\ntitle: 咖啡\ntype: preference\npermalink: 偏好/咖啡\n---\n# 咖啡\n\n## Observations\n- [偏好] 喜欢燕麦拿铁，少糖\n",
            "updatedAt": 1,
        })
    }

    #[test]
    fn parses_the_notes_the_page_sends() {
        let body = json!({ "memory": { "notes": [coffee()] } });
        let loaded = BrowserMemory::parse(&body).unwrap();
        assert_eq!(loaded.skipped, 0);
        assert_eq!(loaded.notes.len(), 1);
        assert_eq!(loaded.notes[0].permalink, "偏好/咖啡");
        assert_eq!(loaded.notes[0].title, "咖啡");
        assert_eq!(
            loaded.notes[0].observations[0].content,
            "喜欢燕麦拿铁，少糖"
        );
        assert!(BrowserMemory::parse(&json!({ "message": "hi" })).is_none());
        let empty = BrowserMemory::parse(&json!({ "memory": {} })).unwrap();
        assert!(empty.notes.is_empty());
    }

    #[test]
    fn invalid_notes_are_skipped() {
        let items = vec![
            coffee(),
            json!({ "permalink": "../etc/passwd", "markdown": "x" }),
            json!({ "permalink": "", "markdown": "x" }),
            json!({ "permalink": "长", "markdown": "x".repeat(MAX_MARKDOWN_CHARS + 1) }),
            json!({ "permalink": 3 }),
        ];
        let loaded = BrowserMemory::parse(&json!({ "memory": { "notes": items } })).unwrap();
        assert_eq!(loaded.notes.len(), 1);
        assert_eq!(loaded.skipped, 4);
    }

    #[test]
    fn excess_notes_are_capped() {
        let items: Vec<Value> = (1..=MAX_NOTES + 3)
            .map(|index| json!({ "permalink": format!("n/{index}"), "markdown": format!("# 第 {index} 条") }))
            .collect();
        let loaded = BrowserMemory::parse(&json!({ "memory": { "notes": items } })).unwrap();
        assert_eq!(loaded.notes.len(), MAX_NOTES);
        assert_eq!(loaded.skipped, 3);
    }

    #[test]
    fn saved_notes_round_trip_through_the_markdown_format() {
        let note = Note {
            observations: vec![Observation {
                category: "地址".into(),
                content: "北京市海淀区".into(),
                tags: Vec::new(),
                context: None,
            }],
            updated_at_millis: 42,
            ..Note::new("家", "places/家")
        };
        let saved = BrowserMemory::saved(&note);
        assert_eq!(saved["action"], "saved");
        let body = json!({ "memory": { "notes": [{
            "permalink": saved["permalink"],
            "markdown": saved["markdown"],
            "updatedAt": 42,
        }] } });
        let parsed = BrowserMemory::parse(&body).unwrap().notes;
        assert_eq!(parsed[0].title, "家");
        assert_eq!(parsed[0].permalink, "places/家");
        assert_eq!(parsed[0].observations[0].content, "北京市海淀区");
        assert_eq!(parsed[0].updated_at_millis, 42);
    }
}
