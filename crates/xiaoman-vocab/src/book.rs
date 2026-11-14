//! The bundled word list and the rules a user word must satisfy.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use serde::Deserialize;
use xiaoman_api::{VocabError, VocabWord};

#[derive(Deserialize)]
struct BookFile {
    words: Vec<VocabWord>,
}

pub struct WordBook {
    words: Vec<VocabWord>,
    index: HashMap<String, usize>,
}

impl WordBook {
    /// The bundled 专升本 list (`data/wordbook.json`), parsed once per process.
    pub fn bundled() -> Arc<WordBook> {
        static BOOK: OnceLock<Arc<WordBook>> = OnceLock::new();
        BOOK.get_or_init(|| {
            Arc::new(
                WordBook::parse(include_str!("../data/wordbook.json"))
                    .expect("the bundled wordbook must parse"),
            )
        })
        .clone()
    }

    pub fn parse(json: &str) -> Result<Self, VocabError> {
        let file: BookFile = serde_json::from_str(json)
            .map_err(|error| VocabError(format!("词库 JSON 无法解析：{error}")))?;
        let mut words = Vec::with_capacity(file.words.len());
        let mut index = HashMap::with_capacity(file.words.len());
        for mut word in file.words {
            validate_word(&word)?;
            let key = normalize_key(&word.word);
            if index.contains_key(&key) {
                return Err(VocabError(format!("词库里有重复单词：{}", word.word)));
            }
            word.word = word.word.trim().to_string();
            index.insert(key, words.len());
            words.push(word);
        }
        if words.is_empty() {
            return Err(VocabError("词库是空的".to_string()));
        }
        Ok(Self { words, index })
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn words(&self) -> &[VocabWord] {
        &self.words
    }

    pub fn get(&self, key: &str) -> Option<&VocabWord> {
        self.index
            .get(&normalize_key(key))
            .map(|index| &self.words[*index])
    }

    pub fn contains(&self, key: &str) -> bool {
        self.index.contains_key(&normalize_key(key))
    }
}

pub fn normalize_key(word: &str) -> String {
    word.trim().to_lowercase()
}

/// Rejects empty or non-English entries, and requires a Chinese meaning.
pub fn validate_word(word: &VocabWord) -> Result<(), VocabError> {
    let text = word.word.trim();
    if text.is_empty() {
        return Err(VocabError("单词不能为空".to_string()));
    }
    if text.chars().count() > 64 {
        return Err(VocabError(format!("单词太长：{text}")));
    }
    let mut has_letter = false;
    for ch in text.chars() {
        if ch.is_ascii_alphabetic() {
            has_letter = true;
        } else if !matches!(ch, '\'' | '-' | ' ') {
            return Err(VocabError(format!(
                "单词「{text}」只能包含英文字母、连字符、撇号或空格"
            )));
        }
    }
    if !has_letter {
        return Err(VocabError(format!("单词「{text}」没有英文字母")));
    }
    if word.meaning.trim().is_empty() {
        return Err(VocabError(format!("单词「{text}」缺少中文释义")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_book_is_complete() {
        let book = WordBook::bundled();
        assert!(book.len() >= 3000, "bundled words: {}", book.len());
        assert!(book.get("abandon").is_some());
        assert!(book.get("state").is_some());
        for word in book.words() {
            assert!(!word.meaning.trim().is_empty(), "{}", word.word);
            assert!(!word.word.trim().is_empty());
        }
    }

    #[test]
    fn rejects_duplicates_and_bad_words() {
        let json = r#"{"words":[
            {"word":"Abandon","meaning":"放弃"},
            {"word":"abandon","meaning":"抛弃"}
        ]}"#;
        assert!(WordBook::parse(json).is_err());
        let json = r#"{"words":[{"word":"中文","meaning":"x"}]}"#;
        assert!(WordBook::parse(json).is_err());
        let json = r#"{"words":[{"word":"ok","meaning":""}]}"#;
        assert!(WordBook::parse(json).is_err());
    }
}
