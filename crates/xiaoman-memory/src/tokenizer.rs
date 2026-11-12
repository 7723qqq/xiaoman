//! Search tokenizer: latin/other words lowercased, CJK runs as unigrams plus bigrams.

use unicode_script::{Script, UnicodeScript};

/// Weight of a CJK unigram taken from a multi-character query run.
pub const WEAK_WEIGHT: f64 = 0.3;

pub fn is_cjk(c: char) -> bool {
    matches!(
        c.script(),
        Script::Han | Script::Hiragana | Script::Katakana | Script::Hangul
    )
}

struct Run {
    text: String,
    cjk: bool,
}

/// Index terms of `text`, with repetitions (for term frequency).
pub fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for run in runs(text) {
        if !run.cjk {
            out.push(run.text);
            continue;
        }
        let chars: Vec<char> = run.text.chars().collect();
        for c in &chars {
            out.push(c.to_string());
        }
        for pair in chars.windows(2) {
            out.push(format!("{}{}", pair[0], pair[1]));
        }
    }
    out
}

/// Query terms with weights; 1.0 for primary terms, [`WEAK_WEIGHT`] for CJK unigrams of longer runs.
pub fn query_terms(text: &str) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    for run in runs(text) {
        if !run.cjk {
            add(&mut out, run.text, 1.0);
            continue;
        }
        let chars: Vec<char> = run.text.chars().collect();
        if chars.len() == 1 {
            add(&mut out, chars[0].to_string(), 1.0);
            continue;
        }
        for pair in chars.windows(2) {
            add(&mut out, format!("{}{}", pair[0], pair[1]), 1.0);
        }
        for c in chars {
            add(&mut out, c.to_string(), WEAK_WEIGHT);
        }
    }
    out
}

fn add(out: &mut Vec<(String, f64)>, term: String, weight: f64) {
    match out.iter_mut().find(|(existing, _)| *existing == term) {
        Some(entry) => {
            if weight > entry.1 {
                entry.1 = weight;
            }
        }
        None => out.push((term, weight)),
    }
}

fn runs(text: &str) -> Vec<Run> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut current_cjk = false;
    for c in text.chars() {
        let cjk = is_cjk(c);
        let word = cjk || c.is_alphanumeric();
        if !word {
            flush(&mut out, &mut current, current_cjk);
            continue;
        }
        if !current.is_empty() && cjk != current_cjk {
            flush(&mut out, &mut current, current_cjk);
        }
        current_cjk = cjk;
        if cjk {
            current.push(c);
        } else {
            current.extend(c.to_lowercase());
        }
    }
    flush(&mut out, &mut current, current_cjk);
    out
}

fn flush(out: &mut Vec<Run>, current: &mut String, cjk: bool) {
    if !current.is_empty() {
        out.push(Run {
            text: std::mem::take(current),
            cjk,
        });
    } else {
        current.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_runs_emit_unigrams_and_bigrams() {
        let expected: Vec<String> = ["家", "的", "地", "址", "家的", "的地", "地址"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(tokens("家的地址"), expected);
    }

    #[test]
    fn latin_runs_are_lowercased_words() {
        let expected: Vec<String> = ["home", "address", "2f"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(tokens("Home Address, 2F"), expected);
    }

    #[test]
    fn query_weights_bigrams_primary_and_unigrams_weak() {
        let terms = query_terms("家的");
        assert_eq!(terms.len(), 3);
        assert_eq!(terms[0], ("家的".to_string(), 1.0));
        assert!(terms.iter().filter(|(_, w)| *w == WEAK_WEIGHT).count() == 2);
    }
}
