//! Session-local notes; nothing is persisted. The browser holds the durable copy
//! (see [`crate::BrowserMemory`]).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use unicode_script::{Script, UnicodeScript};
use xiaoman_api::{MemoryError, MemoryStore, Note, Observation, SearchHit};

pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

pub struct InMemoryMemory {
    notes: Mutex<Vec<(String, Note)>>,
    clock: Clock,
}

impl InMemoryMemory {
    pub fn new(clock: Clock) -> Self {
        Self {
            notes: Mutex::new(Vec::new()),
            clock,
        }
    }

    fn find_in(notes: &[(String, Note)], identifier: &str) -> Option<Note> {
        notes
            .iter()
            .find(|(permalink, _)| permalink == identifier)
            .or_else(|| notes.iter().find(|(_, note)| note.title == identifier))
            .map(|(_, note)| note.clone())
    }

    fn store(&self, note: Note) -> Note {
        let stored = Note {
            updated_at_millis: (self.clock)(),
            ..note
        };
        let mut notes = self.notes.lock().unwrap();
        match notes
            .iter_mut()
            .find(|(permalink, _)| permalink == &stored.permalink)
        {
            Some(entry) => *entry = (stored.permalink.clone(), stored.clone()),
            None => notes.push((stored.permalink.clone(), stored.clone())),
        }
        stored
    }

    pub fn clear(&self) {
        self.notes.lock().unwrap().clear();
    }

    /// Replaces everything with the notes the browser sent for this turn.
    pub fn replace_all(&self, all: Vec<Note>) {
        let mut notes = self.notes.lock().unwrap();
        notes.clear();
        for note in all {
            match notes
                .iter_mut()
                .find(|(permalink, _)| permalink == &note.permalink)
            {
                Some(entry) => *entry = (note.permalink.clone(), note),
                None => notes.push((note.permalink.clone(), note)),
            }
        }
    }

    /// The current note at `permalink`, without lookup by title.
    pub fn snapshot(&self, permalink: &str) -> Option<Note> {
        self.notes
            .lock()
            .unwrap()
            .iter()
            .find(|(key, _)| key == permalink)
            .map(|(_, note)| note.clone())
    }

    pub fn len(&self) -> usize {
        self.notes.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.lock().unwrap().is_empty()
    }
}

#[async_trait]
impl MemoryStore for InMemoryMemory {
    async fn write(&self, note: Note) -> Result<Note, MemoryError> {
        Ok(self.store(note))
    }

    async fn read(&self, identifier: &str) -> Result<Option<Note>, MemoryError> {
        let notes = self.notes.lock().unwrap();
        Ok(Self::find_in(&notes, identifier))
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, MemoryError> {
        let text_query = query.trim().to_lowercase();
        if text_query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let chinese: Vec<char> = text_query
            .chars()
            .filter(|c| c.script() == Script::Han)
            .collect();
        let notes = self.notes.lock().unwrap();
        let mut hits: Vec<SearchHit> = notes
            .iter()
            .filter_map(|(_, note)| {
                let mut parts: Vec<&str> = vec![note.title.as_str(), note.body.as_str()];
                parts.extend(
                    note.observations
                        .iter()
                        .map(|observation| observation.content.as_str()),
                );
                let text = parts.join("\n").to_lowercase();
                let overlap = chinese
                    .iter()
                    .filter(|character| text.contains(**character))
                    .count() as f64
                    / chinese.len().max(1) as f64;
                let score = if text.contains(&text_query) { 2.0 } else { 0.0 }
                    + if note.title.to_lowercase().contains(&text_query) {
                        1.0
                    } else {
                        0.0
                    }
                    + overlap;
                if score == 0.0 {
                    return None;
                }
                Some(SearchHit {
                    permalink: note.permalink.clone(),
                    title: note.title.clone(),
                    snippet: text.chars().take(240).collect(),
                    score,
                })
            })
            .collect();
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        Ok(hits)
    }

    async fn append_observation(
        &self,
        identifier: &str,
        observation: Observation,
    ) -> Result<Note, MemoryError> {
        let notes = self.notes.lock().unwrap();
        let existing =
            Self::find_in(&notes, identifier).unwrap_or_else(|| Note::new(identifier, identifier));
        let body = format!(
            "{}\n- [{}] {}",
            existing.body, observation.category, observation.content
        );
        let mut observations = existing.observations.clone();
        observations.push(observation);
        let note = Note {
            body,
            observations,
            updated_at_millis: (self.clock)(),
            ..existing
        };
        drop(notes);
        Ok(self.store(note))
    }

    async fn delete(&self, identifier: &str) -> Result<bool, MemoryError> {
        let mut notes = self.notes.lock().unwrap();
        let Some(found) = Self::find_in(&notes, identifier) else {
            return Ok(false);
        };
        let before = notes.len();
        notes.retain(|(permalink, _)| permalink != &found.permalink);
        Ok(notes.len() != before)
    }

    async fn list(&self, folder: Option<&str>) -> Result<Vec<Note>, MemoryError> {
        let prefix = folder.map(|folder| format!("{}/", folder.trim_end_matches('/')));
        let notes = self.notes.lock().unwrap();
        Ok(notes
            .iter()
            .filter(|(permalink, _)| match &prefix {
                Some(prefix) => permalink.starts_with(prefix.as_str()),
                None => true,
            })
            .map(|(_, note)| note.clone())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> InMemoryMemory {
        InMemoryMemory::new(Arc::new(|| 123))
    }

    async fn seed(memory: &InMemoryMemory) {
        memory
            .write(Note {
                observations: vec![Observation {
                    category: "地址".into(),
                    content: "中关村".into(),
                    tags: Vec::new(),
                    context: None,
                }],
                body: "Beijing 海淀".into(),
                ..Note::new("家", "places/home")
            })
            .await
            .unwrap();
        memory
            .write(Note::new("工作", "places/work"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn searches_title_body_and_observations_including_chinese_overlap() {
        let memory = memory();
        seed(&memory).await;
        assert_eq!(
            memory.search("BEIJING", 5).await.unwrap()[0].permalink,
            "places/home"
        );
        assert_eq!(memory.search("中关村", 5).await.unwrap()[0].title, "家");
        assert_eq!(memory.search("海淀地址", 5).await.unwrap()[0].title, "家");
        assert_eq!(memory.search("家", 5).await.unwrap()[0].title, "家");
        assert!(memory.search("", 5).await.unwrap().is_empty());
        assert!(memory.search("家", 0).await.unwrap().is_empty());
        assert_eq!(
            memory.read("家").await.unwrap().unwrap().updated_at_millis,
            123
        );
    }

    #[tokio::test]
    async fn supports_replacement_append_read_delete_and_folders() {
        let memory = memory();
        memory.write(Note::new("家", "places/home")).await.unwrap();
        memory
            .write(Note {
                body: "新".into(),
                ..Note::new("家", "places/home")
            })
            .await
            .unwrap();
        memory
            .append_observation(
                "家",
                Observation {
                    category: "地址".into(),
                    content: "北京".into(),
                    tags: Vec::new(),
                    context: None,
                },
            )
            .await
            .unwrap();
        let home = memory.read("places/home").await.unwrap().unwrap();
        assert_eq!(home.observations[0].content, "北京");
        memory
            .append_observation(
                "朋友",
                Observation {
                    category: "姓名".into(),
                    content: "小王".into(),
                    tags: Vec::new(),
                    context: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(memory.list(Some("places")).await.unwrap().len(), 1);
        assert_eq!(memory.list(None).await.unwrap().len(), 2);
        assert!(memory.delete("家").await.unwrap());
        assert!(!memory.delete("家").await.unwrap());
        memory.clear();
        assert!(memory.list(None).await.unwrap().is_empty());
    }
}
