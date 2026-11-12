//! Permalink validation and generation; permalinks are folder-relative paths without `.md`.

use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermalinkError(pub String);

impl fmt::Display for PermalinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for PermalinkError {}

/// Validates `raw` and returns its canonical form: `/` separators, no `.md` suffix, trimmed
/// segments, filesystem-hostile characters replaced by `-`. CJK and other letters are kept.
pub fn normalize(raw: &str) -> Result<String, PermalinkError> {
    let mut path = raw.trim().replace('\\', "/");
    if path.is_empty() {
        return Err(PermalinkError("Permalink is empty".into()));
    }
    if path.starts_with('/') || is_drive_path(&path) {
        return Err(PermalinkError(format!("Permalink must be relative: {raw}")));
    }
    if path.to_lowercase().ends_with(".md") {
        path.truncate(path.len() - 3);
    }
    let mut segments = Vec::new();
    for segment in path.split('/') {
        let segment = segment.trim();
        if segment.is_empty() {
            return Err(PermalinkError(format!(
                "Permalink has an empty segment: {raw}"
            )));
        }
        if segment == "." || segment == ".." {
            return Err(PermalinkError(format!(
                "Permalink must not contain . or ..: {raw}"
            )));
        }
        if segment.starts_with('.') {
            return Err(PermalinkError(format!(
                "Permalink segments must not start with '.': {raw}"
            )));
        }
        segments.push(sanitize(segment));
    }
    Ok(segments.join("/"))
}

/// Slug for `title`: latin lowercased, letters/digits (incl. CJK) kept, everything else `-`.
pub fn from_title(title: &str) -> String {
    let mut slug = String::new();
    for c in title.trim().to_lowercase().chars() {
        if c.is_alphanumeric() || c == '_' {
            slug.push(c);
        } else {
            slug.push('-');
        }
    }
    let mut collapsed = String::with_capacity(slug.len());
    let mut previous_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if previous_dash {
                continue;
            }
            previous_dash = true;
        } else {
            previous_dash = false;
        }
        collapsed.push(c);
    }
    collapsed.trim_matches('-').to_string()
}

fn is_drive_path(path: &str) -> bool {
    let mut chars = path.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

fn sanitize(segment: &str) -> String {
    segment
        .chars()
        .map(|c| {
            if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|')
                || (c as u32) < 0x20
                || c == '\u{7f}'
            {
                '-'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_rejects() {
        assert_eq!(normalize(" places/家.md ").unwrap(), "places/家");
        assert_eq!(normalize(r"a\b").unwrap(), "a/b");
        assert_eq!(normalize("ab:cd").unwrap(), "ab-cd");
        for bad in [
            "",
            "/abs",
            "C:/x",
            "x:y",
            "a//b",
            "a/./b",
            "a/",
            ".hidden/x",
            "../x",
            "a/../../b",
        ] {
            assert!(normalize(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn slugs_keep_cjk_and_collapse_dashes() {
        assert_eq!(from_title("家的地址"), "家的地址");
        assert_eq!(from_title("Taxi: Home  v2!"), "taxi-home-v2");
        assert_eq!(from_title("  ---  "), "");
    }
}
