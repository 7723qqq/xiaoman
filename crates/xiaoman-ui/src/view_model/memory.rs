//! The memory screen's rows: `MemoryScreen.kt` over `xiaoman_api::{Note, SearchHit}`.
//!
//! The Kotlin `MemoryViewModel` owns asynchrony (debounced query, coroutine refresh). Here the
//! store is injected by the caller and this module keeps only the deterministic mapping and the
//! screen's small state machine: query → rows, open → note, delete → back to the list.

use xiaoman_api::{Note, SearchHit};
use xiaoman_app::chat_item::{RichText, chat_rich_text};

use super::copy;

/// A row of the memory list: every note when the query is blank, search hits otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRow {
    pub permalink: String,
    pub title: String,
    pub detail: String,
}

/// The opened note, rendered as the text the screen paints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenNote {
    pub permalink: String,
    pub title: String,
    pub body: RichText,
}

/// The list state `MemoryState` holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryState {
    pub query: String,
    pub rows: Vec<MemoryRow>,
    pub loading: bool,
    pub open: Option<OpenNote>,
    pub open_missing: bool,
    /// The note whose deletion is waiting for confirmation.
    pub confirm_delete: Option<String>,
    /// `已删除。` or why the delete failed, shown above the list.
    pub message: Option<String>,
}

impl Default for MemoryState {
    fn default() -> Self {
        Self {
            query: String::new(),
            rows: Vec::new(),
            loading: true,
            open: None,
            open_missing: false,
            confirm_delete: None,
            message: None,
        }
    }
}

impl MemoryState {
    /// `memory_empty` or `memory_no_hits`, depending on the query.
    pub fn empty_hint(&self) -> Option<&'static str> {
        if self.loading || !self.rows.is_empty() {
            return None;
        }
        Some(if self.query.trim().is_empty() {
            copy::MEMORY_EMPTY
        } else {
            copy::MEMORY_NO_HITS
        })
    }

    pub fn not_found(&self) -> Option<&'static str> {
        self.open_missing.then_some(copy::MEMORY_NOT_FOUND)
    }
}

/// `memory.list()`: newest first, the permalink as the supporting line.
pub fn note_rows(notes: &[Note]) -> Vec<MemoryRow> {
    let mut sorted: Vec<&Note> = notes.iter().collect();
    sorted.sort_by_key(|note| std::cmp::Reverse(note.updated_at_millis));
    sorted
        .into_iter()
        .map(|note| MemoryRow {
            permalink: note.permalink.clone(),
            title: note.title.clone(),
            detail: note.permalink.clone(),
        })
        .collect()
}

/// `memory.search(q, limit = 50)`: the snippet flattened onto one line.
pub fn hit_rows(hits: &[SearchHit]) -> Vec<MemoryRow> {
    hits.iter()
        .map(|hit| MemoryRow {
            permalink: hit.permalink.clone(),
            title: hit.title.clone(),
            detail: hit.snippet.replace('\n', " "),
        })
        .collect()
}

/// `BasicMemoryFormat.render`, reduced to the emphasis and list markers the chat renderer
/// already understands: heading markers dropped, `**bold**` to spans, `- ` to `• `.
pub fn render_note(note: &Note) -> OpenNote {
    OpenNote {
        permalink: note.permalink.clone(),
        title: note.title.clone(),
        body: chat_rich_text(&note.body),
    }
}

/// The screen's state machine, driven by the caller instead of a coroutine scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryModel {
    state: MemoryState,
}

impl MemoryModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> &MemoryState {
        &self.state
    }

    pub fn query(&self) -> &str {
        &self.state.query
    }

    pub fn rows(&self) -> &[MemoryRow] {
        &self.state.rows
    }

    pub fn open(&self) -> Option<&OpenNote> {
        self.state.open.as_ref()
    }

    pub fn loading(&self) -> bool {
        self.state.loading
    }

    /// `setQuery`: remember the text and mark the list as still loading.
    pub fn set_query(&mut self, text: impl Into<String>) {
        self.state.query = text.into();
        self.state.loading = true;
    }

    pub fn begin_refresh(&mut self) {
        self.state.loading = true;
    }

    /// The blank query lists every note; a query lists its hits.
    pub fn set_notes(&mut self, notes: &[Note]) {
        self.state.rows = note_rows(notes);
        self.state.loading = false;
    }

    pub fn set_hits(&mut self, hits: &[SearchHit]) {
        self.state.rows = hit_rows(hits);
        self.state.loading = false;
    }

    /// A failed list or search shows an empty list, as `runCatching{}.getOrDefault` does.
    pub fn set_failed(&mut self) {
        self.state.rows.clear();
        self.state.loading = false;
    }

    pub fn open_note(&mut self, note: &Note) {
        self.state.open = Some(render_note(note));
        self.state.open_missing = false;
    }

    pub fn open_missing(&mut self) {
        self.state.open = None;
        self.state.open_missing = true;
    }

    pub fn close(&mut self) {
        self.state.open = None;
        self.state.open_missing = false;
        self.state.confirm_delete = None;
    }

    /// Delete: the note closes and the list reloads.
    pub fn deleted(&mut self) {
        self.close();
        self.state.loading = true;
    }

    /// The note the delete confirmation is about, if the dialog is open.
    pub fn confirm_delete(&self) -> Option<&str> {
        self.state.confirm_delete.as_deref()
    }

    /// The trash button: ask before touching the file.
    pub fn begin_delete(&mut self, permalink: impl Into<String>) {
        self.state.confirm_delete = Some(permalink.into());
    }

    pub fn cancel_delete(&mut self) {
        self.state.confirm_delete = None;
    }

    pub fn message(&self) -> Option<&str> {
        self.state.message.as_deref()
    }

    pub fn set_message(&mut self, message: Option<String>) {
        self.state.message = message;
    }
}
