//! `MemoryStore` backed by a folder of Basic Memory Markdown files.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use async_trait::async_trait;
use xiaoman_api::{MemoryError, MemoryStore, Note, Observation, SearchHit};

use crate::format::BasicMemoryFormat;
use crate::index::MemoryIndex;
use crate::permalinks::{from_title, normalize};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Note `a/b` lives in `<root>/a/b.md`.
///
/// Files may be added or edited by other tools (adb push, Obsidian); every lookup rescans the folder
/// by mtime/size so the search index follows them. Writes are atomic (temp file + rename), UTF-8,
/// and stamp the file's mtime with the configured clock. Hidden files and folders are ignored.
pub struct FileMemoryStore {
    root: PathBuf,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    index: Mutex<MemoryIndex>,
}

impl FileMemoryStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_clock(root, Arc::new(xiaoman_api::now_millis))
    }

    pub fn with_clock(root: impl Into<PathBuf>, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        let root = root.into();
        Self {
            index: Mutex::new(MemoryIndex::new(root.clone())),
            root,
            clock,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn write_locked(&self, index: &mut MemoryIndex, note: Note) -> Result<Note, MemoryError> {
        let title = note.title.trim();
        let title = if title.is_empty() {
            note.permalink
                .trim()
                .trim_end_matches(".md")
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        } else {
            title.to_string()
        };
        if title.is_empty() {
            return Err(MemoryError("Note needs a title or a permalink".into()));
        }
        let permalink = if note.permalink.trim().is_empty() {
            self.generate_permalink(index, &title)?
        } else {
            normalize(&note.permalink).map_err(|error| MemoryError(error.0))?
        };
        let file = self.file_for(&permalink)?;
        let mut stored_note = note;
        stored_note.title = title;
        stored_note.permalink = permalink.clone();
        let text = BasicMemoryFormat::render(&stored_note);
        atomic_write(&file, &text)?;
        set_modified(&file, (self.clock)())?;
        let mtime = modified_millis(&file);
        let size = fs::metadata(&file)
            .map(|meta| meta.len())
            .unwrap_or_default();
        let stored = BasicMemoryFormat::parse(&text, &permalink, mtime);
        index.put(stored.clone(), mtime, size);
        Ok(stored)
    }

    fn read_locked(
        &self,
        index: &mut MemoryIndex,
        identifier: &str,
    ) -> Result<Option<Note>, MemoryError> {
        let key = identifier.trim();
        if key.is_empty() {
            return Ok(None);
        }
        if let Ok(permalink) = normalize(key) {
            let file = self.file_for(&permalink)?;
            if file.is_file() {
                return Ok(index.load(&permalink, &file));
            }
        }
        index.sync();
        let mut notes = index.notes();
        notes.sort_by(|a, b| a.permalink.cmp(&b.permalink));
        if let Some(exact) = notes.iter().find(|note| note.title == key) {
            return Ok(Some(exact.clone()));
        }
        let lower = key.to_lowercase();
        Ok(notes
            .into_iter()
            .find(|note| note.title.to_lowercase() == lower))
    }

    /// Slug of `title`, suffixed `-2`, `-3`, … when another note already owns it.
    fn generate_permalink(
        &self,
        index: &mut MemoryIndex,
        title: &str,
    ) -> Result<String, MemoryError> {
        let slug = from_title(title);
        let base = if slug.is_empty() {
            format!("note-{}", (self.clock)())
        } else {
            slug
        };
        let mut candidate = base.clone();
        let mut suffix = 2;
        loop {
            let file = self.file_for(&candidate)?;
            if !file.exists() {
                return Ok(candidate);
            }
            if file.is_file() {
                if let Some(note) = index.load(&candidate, &file) {
                    if note.title == title {
                        return Ok(candidate);
                    }
                }
            }
            candidate = format!("{base}-{suffix}");
            suffix += 1;
        }
    }

    fn file_for(&self, permalink: &str) -> Result<PathBuf, MemoryError> {
        let file = self.root.join(format!("{permalink}.md"));
        // normalize() rejects "..", absolute paths and hidden segments, so this is a lexical guard.
        if !file.starts_with(&self.root) {
            return Err(MemoryError(format!(
                "Permalink escapes the memory folder: {permalink}"
            )));
        }
        Ok(file)
    }
}

#[async_trait]
impl MemoryStore for FileMemoryStore {
    async fn write(&self, note: Note) -> Result<Note, MemoryError> {
        let mut index = self.index.lock().unwrap();
        self.write_locked(&mut index, note)
    }

    async fn read(&self, identifier: &str) -> Result<Option<Note>, MemoryError> {
        let mut index = self.index.lock().unwrap();
        self.read_locked(&mut index, identifier)
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, MemoryError> {
        let mut index = self.index.lock().unwrap();
        index.sync();
        Ok(index.search(query, limit))
    }

    async fn append_observation(
        &self,
        identifier: &str,
        observation: Observation,
    ) -> Result<Note, MemoryError> {
        let mut index = self.index.lock().unwrap();
        if let Some(mut existing) = self.read_locked(&mut index, identifier)? {
            existing.observations.push(observation);
            return self.write_locked(&mut index, existing);
        }
        let permalink = normalize(identifier).map_err(|error| MemoryError(error.0))?;
        let title = permalink
            .rsplit('/')
            .next()
            .unwrap_or(&permalink)
            .to_string();
        let mut note = Note::new(title, permalink);
        note.observations.push(observation);
        self.write_locked(&mut index, note)
    }

    async fn delete(&self, identifier: &str) -> Result<bool, MemoryError> {
        let mut index = self.index.lock().unwrap();
        let Some(note) = self.read_locked(&mut index, identifier)? else {
            return Ok(false);
        };
        let file = self.file_for(&note.permalink)?;
        let deleted = fs::remove_file(&file).is_ok();
        index.remove(&note.permalink);
        prune_empty_folders(&self.root, file.parent());
        Ok(deleted)
    }

    async fn list(&self, folder: Option<&str>) -> Result<Vec<Note>, MemoryError> {
        let mut index = self.index.lock().unwrap();
        index.sync();
        let prefix = folder
            .map(|folder| {
                folder
                    .trim()
                    .replace('\\', "/")
                    .trim_matches('/')
                    .to_string()
            })
            .filter(|folder| !folder.is_empty())
            .map(|folder| format!("{folder}/"));
        let mut notes: Vec<Note> = index
            .notes()
            .into_iter()
            .filter(|note| {
                prefix
                    .as_ref()
                    .is_none_or(|prefix| note.permalink.starts_with(prefix))
            })
            .collect();
        notes.sort_by(|a, b| a.permalink.cmp(&b.permalink));
        Ok(notes)
    }
}

fn atomic_write(file: &Path, text: &str) -> Result<(), MemoryError> {
    if let Some(dir) = file.parent() {
        if !dir.is_dir() {
            fs::create_dir_all(dir)?;
        }
    }
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "note".to_string());
    let unique = format!(
        ".{}.{}.{}.tmp",
        name,
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let temp = file.with_file_name(unique);
    let result = (|| -> std::io::Result<()> {
        let mut handle = fs::File::create(&temp)?;
        handle.write_all(text.as_bytes())?;
        handle.sync_all()?;
        drop(handle);
        fs::rename(&temp, file)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(MemoryError::from)
}

fn set_modified(file: &Path, millis: i64) -> Result<(), MemoryError> {
    let time = UNIX_EPOCH + Duration::from_millis(millis.max(0) as u64);
    let handle = fs::OpenOptions::new().write(true).open(file)?;
    handle.set_modified(time)?;
    Ok(())
}

fn modified_millis(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

fn prune_empty_folders(root: &Path, start: Option<&Path>) {
    let Some(mut dir) = start.map(Path::to_path_buf) else {
        return;
    };
    while dir != root && dir.starts_with(root) {
        let empty = fs::read_dir(&dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !empty || fs::remove_dir(&dir).is_err() {
            return;
        }
        dir = match dir.parent() {
            Some(parent) => parent.to_path_buf(),
            None => return,
        };
    }
}

/// A clock that advances by a fixed step on every read, for tests.
pub fn stepping_clock(start: i64, step: i64) -> Arc<dyn Fn() -> i64 + Send + Sync> {
    let now = std::sync::atomic::AtomicI64::new(start);
    Arc::new(move || now.fetch_add(step, Ordering::SeqCst))
}
