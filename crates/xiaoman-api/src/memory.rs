//! Basic Memory notes: a folder of Markdown files that also opens in Obsidian.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A note in the Basic Memory format:
///
/// ```text
/// ---
/// title: 家
/// type: place
/// permalink: places/家
/// tags: [地址]
/// ---
/// # 家
///
/// ## Observations
/// - [address] 北京市海淀区某某路 1 号 #常用 (用户 2026-10-30 告知)
///
/// ## Relations
/// - lives_in [[北京]]
/// ```
///
/// Observation lines are `- [category] content #tag ... (optional context)`.
/// Relation lines are `- relation_type [[Target Title]] (optional context)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub title: String,
    /// Folder-relative path without `.md`, e.g. `places/家`. Unique key of the note.
    pub permalink: String,
    #[serde(rename = "type", default = "default_note_type")]
    pub note_type: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Frontmatter keys other than title/type/permalink/tags, kept on rewrite.
    #[serde(default)]
    pub extra_frontmatter: serde_json::Map<String, serde_json::Value>,
    /// Markdown body after the frontmatter, including the Observations/Relations sections.
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub observations: Vec<Observation>,
    #[serde(default)]
    pub relations: Vec<Relation>,
    #[serde(default)]
    pub updated_at_millis: i64,
}

fn default_note_type() -> String {
    "note".to_string()
}

impl Note {
    pub fn new(title: impl Into<String>, permalink: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            permalink: permalink.into(),
            note_type: default_note_type(),
            tags: Vec::new(),
            extra_frontmatter: Default::default(),
            body: String::new(),
            observations: Vec::new(),
            relations: Vec::new(),
            updated_at_millis: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub category: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    #[serde(rename = "type")]
    pub relation_type: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub permalink: String,
    pub title: String,
    pub snippet: String,
    pub score: f64,
}

#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Creates or replaces the note at [`Note::permalink`]; returns the stored note.
    async fn write(&self, note: Note) -> Result<Note, MemoryError>;

    /// Looks a note up by permalink, then by exact title.
    async fn read(&self, identifier: &str) -> Result<Option<Note>, MemoryError>;

    /// Full-text search over titles, observations and bodies; must handle Chinese text.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, MemoryError>;

    /// Adds one observation, creating the note (type `note`) when it does not exist.
    async fn append_observation(
        &self,
        identifier: &str,
        observation: Observation,
    ) -> Result<Note, MemoryError>;

    /// Real deletion of the file and its index entries. Returns false when nothing was there.
    async fn delete(&self, identifier: &str) -> Result<bool, MemoryError>;

    async fn list(&self, folder: Option<&str>) -> Result<Vec<Note>, MemoryError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryError(pub String);

impl std::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MemoryError {}

impl From<std::io::Error> for MemoryError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}
