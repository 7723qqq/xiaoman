//! In-memory inverted index over the note files under a root folder.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use xiaoman_api::{Note, SearchHit};

use crate::format::BasicMemoryFormat;
use crate::tokenizer::{query_terms, tokens};

const K1: f64 = 1.2;
const B: f64 = 0.75;
const TITLE_BOOST: f64 = 3.0;
const TAG_BOOST: f64 = 2.0;
const TITLE_PHRASE_FACTOR: f64 = 1.5;
const BODY_PHRASE_FACTOR: f64 = 1.2;

struct Doc {
    note: Note,
    mtime: i64,
    size: u64,
    terms: HashMap<String, f64>,
    length: usize,
}

/// In-memory inverted index over the note files under a root folder. Not thread-safe; the owning
/// store serialises access. [`MemoryIndex::sync`] picks up files added, changed or removed by
/// other tools.
pub struct MemoryIndex {
    root: PathBuf,
    docs: HashMap<String, Doc>,
    document_frequency: Option<HashMap<String, usize>>,
    average_length: f64,
}

impl MemoryIndex {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            docs: HashMap::new(),
            document_frequency: None,
            average_length: 1.0,
        }
    }

    pub fn notes(&self) -> Vec<Note> {
        self.docs.values().map(|doc| doc.note.clone()).collect()
    }

    /// Rescans the root, re-parsing files whose mtime or size changed and dropping vanished ones.
    pub fn sync(&mut self) {
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        if self.root.is_dir() {
            walk(&self.root, &self.root, &mut files);
        }
        for (permalink, path) in files {
            seen.insert(permalink.clone());
            self.load(&permalink, &path);
        }
        let before = self.docs.len();
        self.docs.retain(|permalink, _| seen.contains(permalink));
        if self.docs.len() != before {
            self.document_frequency = None;
        }
    }

    /// Returns the note in `file`, re-reading it only when it changed since it was indexed.
    pub fn load(&mut self, permalink: &str, file: &Path) -> Option<Note> {
        let mtime = modified_millis(file);
        let size = fs::metadata(file)
            .map(|meta| meta.len())
            .unwrap_or_default();
        if let Some(doc) = self.docs.get(permalink) {
            if doc.mtime == mtime && doc.size == size && file.is_file() {
                return Some(doc.note.clone());
            }
        }
        if !file.is_file() {
            self.remove(permalink);
            return None;
        }
        let text = fs::read_to_string(file).ok()?;
        let note = BasicMemoryFormat::parse(&text, permalink, mtime);
        self.put(note.clone(), mtime, size);
        Some(note)
    }

    pub fn put(&mut self, note: Note, mtime: i64, size: u64) {
        let mut terms: HashMap<String, f64> = HashMap::new();
        let mut add = |text: &str, weight: f64| {
            for token in tokens(text) {
                *terms.entry(token).or_insert(0.0) += weight;
            }
        };
        add(&note.body, 1.0);
        add(&note.title, TITLE_BOOST);
        for tag in &note.tags {
            add(tag, TAG_BOOST);
        }
        let length = tokens(&note.body).len() + tokens(&note.title).len();
        self.docs.insert(
            note.permalink.clone(),
            Doc {
                note,
                mtime,
                size,
                terms,
                length,
            },
        );
        self.document_frequency = None;
    }

    pub fn remove(&mut self, permalink: &str) {
        if self.docs.remove(permalink).is_some() {
            self.document_frequency = None;
        }
    }

    /// BM25 search; a blank `query` returns the most recently updated notes.
    pub fn search(&mut self, query: &str, limit: usize) -> Vec<SearchHit> {
        if limit == 0 {
            return Vec::new();
        }
        let trimmed = query.trim();
        if trimmed.is_empty() {
            let mut docs: Vec<&Doc> = self.docs.values().collect();
            docs.sort_by(|a, b| {
                b.note
                    .updated_at_millis
                    .cmp(&a.note.updated_at_millis)
                    .then_with(|| a.note.permalink.cmp(&b.note.permalink))
            });
            return docs
                .into_iter()
                .take(limit)
                .map(|doc| SearchHit {
                    permalink: doc.note.permalink.clone(),
                    title: doc.note.title.clone(),
                    snippet: Snippets::excerpt(&doc.note),
                    score: 0.0,
                })
                .collect();
        }
        let terms = query_terms(trimmed);
        if terms.is_empty() {
            return Vec::new();
        }
        self.ensure_frequencies();
        let frequencies = self
            .document_frequency
            .as_ref()
            .expect("frequencies computed");
        let total_documents = self.docs.len() as f64;
        let average_length = self.average_length;
        let phrase = trimmed.to_lowercase();

        let mut scored: Vec<(&Doc, f64, bool)> = Vec::new();
        for doc in self.docs.values() {
            let mut score = 0.0;
            let mut primary = false;
            for (term, weight) in &terms {
                let Some(tf) = doc.terms.get(term) else {
                    continue;
                };
                let Some(docs_with_term) = frequencies.get(term) else {
                    continue;
                };
                let idf = (1.0
                    + (total_documents - *docs_with_term as f64 + 0.5)
                        / (*docs_with_term as f64 + 0.5))
                    .ln();
                score += weight * idf * tf * (K1 + 1.0)
                    / (tf + K1 * (1.0 - B + B * doc.length as f64 / average_length));
                if *weight >= 1.0 {
                    primary = true;
                }
            }
            if score <= 0.0 {
                continue;
            }
            if doc.note.title.to_lowercase().contains(&phrase) {
                score *= TITLE_PHRASE_FACTOR;
            } else if doc.note.body.to_lowercase().contains(&phrase) {
                score *= BODY_PHRASE_FACTOR;
            }
            scored.push((doc, score, primary));
        }
        // Weak CJK unigram matches only rank notes that also match a primary term; on their own they
        // are noise (e.g. "你好" would pull in every note containing 好).
        let mut kept: Vec<(&Doc, f64)> = scored
            .into_iter()
            .filter(|(_, _, primary)| *primary)
            .map(|(doc, score, _)| (doc, score))
            .collect();
        kept.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.0.note.updated_at_millis.cmp(&a.0.note.updated_at_millis))
        });
        kept.into_iter()
            .take(limit)
            .map(|(doc, score)| SearchHit {
                permalink: doc.note.permalink.clone(),
                title: doc.note.title.clone(),
                snippet: Snippets::best(&doc.note, &terms),
                score,
            })
            .collect()
    }

    fn ensure_frequencies(&mut self) {
        if self.document_frequency.is_some() {
            return;
        }
        let mut frequencies: HashMap<String, usize> = HashMap::new();
        let mut total = 0usize;
        for doc in self.docs.values() {
            for term in doc.terms.keys() {
                *frequencies.entry(term.clone()).or_insert(0) += 1;
            }
            total += doc.length;
        }
        self.average_length = if self.docs.is_empty() {
            1.0
        } else {
            (total as f64 / self.docs.len() as f64).max(1.0)
        };
        self.document_frequency = Some(frequencies);
    }
}

/// Picks the line of a note that best explains a hit.
pub struct Snippets;

impl Snippets {
    pub const MAX_LENGTH: usize = 120;

    /// The body line (observations preferred on ties) matching the most query weight, clipped.
    pub fn best(note: &Note, terms: &[(String, f64)]) -> String {
        let lines = candidates(note);
        let mut best: Option<&str> = None;
        let mut best_score = 0.0;
        for line in &lines {
            let line_tokens: HashSet<String> = tokens(line).into_iter().collect();
            let mut score: f64 = terms
                .iter()
                .filter(|(term, _)| line_tokens.contains(term))
                .map(|(_, weight)| *weight)
                .sum();
            if line.starts_with('[') {
                score += 0.01;
            }
            if score > best_score + 1e-9 && score > 0.01 {
                best = Some(line);
                best_score = score;
            }
        }
        let chosen = best
            .map(str::to_string)
            .or_else(|| lines.first().cloned())
            .unwrap_or_else(|| note.title.clone());
        let terms: Vec<String> = terms.iter().map(|(term, _)| term.clone()).collect();
        clip(&chosen, &terms)
    }

    /// First meaningful body line, for listings without a query.
    pub fn excerpt(note: &Note) -> String {
        let chosen = candidates(note)
            .first()
            .cloned()
            .unwrap_or_else(|| note.title.clone());
        clip(&chosen, &[])
    }
}

fn candidates(note: &Note) -> Vec<String> {
    note.body
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && !line.starts_with('#') && *line != "---" && !line.starts_with("```")
        })
        .map(strip_list_marker)
        .filter(|line| !line.is_empty())
        .collect()
}

static LIST_MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-*+]\s+").unwrap());

/// The Kotlin `Snippets.LIST_MARKER` is `^[-*+]\s+`, so tabs and repeated spaces are stripped too.
fn strip_list_marker(line: &str) -> String {
    LIST_MARKER.replace(line, "").to_string()
}

fn clip(line: &str, terms: &[String]) -> String {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= Snippets::MAX_LENGTH {
        return line.to_string();
    }
    let lower = line.to_lowercase();
    let hit_byte = terms
        .iter()
        .filter_map(|term| lower.find(term))
        .min()
        .unwrap_or(0);
    let hit = lower[..hit_byte].chars().count();
    let start = hit
        .saturating_sub(30)
        .min(chars.len() - Snippets::MAX_LENGTH);
    let end = start + Snippets::MAX_LENGTH;
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < chars.len() { "…" } else { "" };
    format!(
        "{prefix}{}{suffix}",
        chars[start..end].iter().collect::<String>()
    )
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if let Some(stem) = name.strip_suffix(".md") {
            let _ = stem;
            if let Ok(relative) = path.strip_prefix(root) {
                let permalink = relative.to_string_lossy().replace('\\', "/");
                let permalink = permalink.trim_end_matches(".md").to_string();
                out.push((permalink, path));
            }
        }
    }
}

fn modified_millis(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::strip_list_marker;

    #[test]
    fn list_markers_follow_the_kotlin_regex() {
        assert_eq!("项目", strip_list_marker("-   项目"));
        assert_eq!("项目", strip_list_marker("-	项目"));
        assert_eq!("项目", strip_list_marker("* 项目"));
        assert_eq!("项目", strip_list_marker("+ 项目"));
        assert_eq!("no marker", strip_list_marker("no marker"));
        assert_eq!("-no-space", strip_list_marker("-no-space"));
    }
}
