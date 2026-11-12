//! Confirmation-word matching and URI scheme/host comparison.

use std::fmt;

/// Built-in confirmation words. Skills and extra words can only add to them.
pub const BUILT_IN_CONFIRM_WORDS: &[&str] = &[
    "支付",
    "付款",
    "确认支付",
    "立即支付",
    "转账",
    "发送",
    "删除",
    "下单",
    "提交订单",
    "确认呼叫",
    "呼叫",
    "购买",
    "pay",
    "send",
    "delete",
    "purchase",
    "order",
];

/// Words are matched as substrings, except that an end of the word made of a latin letter or digit
/// must sit at a word boundary, case-insensitively: "order" matches "Place Order" but not "border";
/// "支付" matches "立即支付". A Chinese word right after "人" or "人已" is a shopping counter, not a
/// button: "付款" does not match "1000+人付款" or "39人已购买", but still matches "立即付款".
pub fn matches(word: &str, text: &str) -> bool {
    let word: Vec<char> = word.chars().collect();
    if word.is_empty() {
        return false;
    }
    let text: Vec<char> = text.chars().collect();
    if word.len() > text.len() {
        return false;
    }
    let head_is_latin = is_latin_word_char(word[0]);
    let tail_is_latin = is_latin_word_char(word[word.len() - 1]);
    for start in 0..=(text.len() - word.len()) {
        let end = start + word.len();
        if !eq_ignore_case(&text[start..end], &word) {
            continue;
        }
        if head_is_latin {
            if start > 0 && is_latin_word_char(text[start - 1]) {
                continue;
            }
        } else {
            if start > 0 && text[start - 1] == '人' {
                continue;
            }
            if start >= 2 && text[start - 2] == '人' && text[start - 1] == '已' {
                continue;
            }
        }
        if tail_is_latin && end < text.len() && is_latin_word_char(text[end]) {
            continue;
        }
        return true;
    }
    false
}

/// The longest of `words` found in `text`, or `None`. Ties keep the first spelling, as `maxByOrNull` does.
pub fn longest_in<'a>(text: &str, words: &'a [String]) -> Option<&'a str> {
    let mut best: Option<&'a str> = None;
    for word in words {
        if !matches(word, text) {
            continue;
        }
        let longer = match best {
            None => true,
            Some(current) => word.chars().count() > current.chars().count(),
        };
        if longer {
            best = Some(word.as_str());
        }
    }
    best
}

/// Trims, drops blanks and duplicates (case-insensitively), keeping the first spelling.
pub fn normalize(words: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    for word in words {
        let trimmed = word.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        kept.push(trimmed.to_string());
    }
    kept
}

fn is_latin_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

fn eq_ignore_case(text: &[char], word: &[char]) -> bool {
    text.iter().zip(word).all(|(a, b)| {
        if a.is_ascii() && b.is_ascii() {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    })
}

/// Scheme and host of a URI or a deeplink template (templates may hold `{placeholders}`), lowercased.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemeHost {
    pub scheme: String,
    pub host: String,
}

impl fmt::Display for SchemeHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.is_empty() {
            write!(f, "{}:", self.scheme)
        } else {
            write!(f, "{}://{}", self.scheme, self.host)
        }
    }
}

impl SchemeHost {
    pub fn of(uri: &str) -> Option<Self> {
        let trimmed = uri.trim();
        let colon = trimmed.find(':')?;
        if colon == 0 {
            return None;
        }
        let scheme = &trimmed[..colon];
        if !scheme
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            return None;
        }
        let rest = &trimmed[colon + 1..];
        let host = if let Some(after) = rest.strip_prefix("//") {
            let authority: String = after
                .chars()
                .take_while(|c| !matches!(c, '/' | '?' | '#'))
                .collect();
            let host_part = authority
                .rsplit_once('@')
                .map(|(_, host)| host)
                .unwrap_or(&authority);
            let host_part = host_part.split(':').next().unwrap_or_default();
            host_part.to_lowercase()
        } else {
            String::new()
        };
        Some(Self {
            scheme: scheme.to_lowercase(),
            host,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words() -> Vec<String> {
        BUILT_IN_CONFIRM_WORDS
            .iter()
            .map(|w| (*w).to_string())
            .collect()
    }

    #[test]
    fn shopping_counters_are_not_confirm_words() {
        let words = words();
        for text in ["¥4.17 1000+人付款", "39人已购买", "2万人购买 包邮"] {
            assert_eq!(longest_in(text, &words), None, "{text}");
        }
        assert_eq!(longest_in("1000+人付款 立即付款", &words), Some("付款"));
        assert_eq!(longest_in("立即购买", &words), Some("购买"));
        assert_eq!(longest_in("确认支付 ¥2.35", &words), Some("确认支付"));
    }

    #[test]
    fn latin_words_need_boundaries() {
        let words = words();
        assert_eq!(longest_in("Place ORDER", &words), Some("order"));
        assert_eq!(longest_in("border", &words), None);
        assert_eq!(longest_in("sender name", &words), None);
        assert_eq!(longest_in("Payment history", &words), None);
        assert_eq!(longest_in("点击send按钮", &words), Some("send"));
    }

    #[test]
    fn scheme_and_host_are_compared_case_insensitively() {
        assert_eq!(
            SchemeHost::of("AMAPURI://Route/other").map(|s| s.to_string()),
            Some("amapuri://route".to_string())
        );
        assert_eq!(
            SchemeHost::of("didi://route/plan").map(|s| s.to_string()),
            Some("didi://route".to_string())
        );
        assert_eq!(SchemeHost::of("not a uri"), None);
        assert_eq!(
            SchemeHost::of("https://user@Host.example:8080/path").map(|s| s.to_string()),
            Some("https://host.example".to_string())
        );
        assert_eq!(
            SchemeHost::of("file:/tmp/x").map(|s| s.to_string()),
            Some("file:".to_string())
        );
    }
}
