//! Reads and writes notes in the Basic Memory Markdown format.
//!
//! Observations and relations are parsed from list lines anywhere in the body outside fenced code;
//! other `[[wiki links]]` become `links_to` relations.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};
use xiaoman_api::{Note, Observation, Relation};

/// Relation type given to wiki links outside relation lines.
pub const LINKS_TO: &str = "links_to";

const RESERVED: [&str; 4] = ["title", "type", "permalink", "tags"];

static OBSERVATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-*+]\s+\[([^\[\]]+)](?:\s+(.*))?$").unwrap());
static RELATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[-*+]\s+([^\[\]()#\s][^\[\]()#]*?)\s+\[\[([^\[\]]+)]]\s*(?:\((.*)\))?$").unwrap()
});
static WIKI_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\[\]]+)]]").unwrap());
static INLINE_CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`[^`]*`").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static SECTION_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,2}\s.*").unwrap());

pub struct BasicMemoryFormat;

impl BasicMemoryFormat {
    /// Parses a note file; `permalink` is the file's path key and `mtime` becomes `updated_at_millis`.
    pub fn parse(text: &str, permalink: &str, mtime: i64) -> Note {
        let (frontmatter, body) = split_frontmatter(text);
        let (observations, relations) = parse_body(&body);
        let title = frontmatter
            .get("title")
            .and_then(json_text)
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
            .or_else(|| first_heading(&body))
            .unwrap_or_else(|| {
                permalink
                    .rsplit('/')
                    .next()
                    .unwrap_or(permalink)
                    .to_string()
            });
        let note_type = frontmatter
            .get("type")
            .and_then(json_text)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "note".to_string());
        let tags = parse_tags(frontmatter.get("tags"));
        let extra_frontmatter = frontmatter
            .into_iter()
            .filter(|(key, _)| !RESERVED.contains(&key.as_str()))
            .collect();
        Note {
            title,
            permalink: permalink.to_string(),
            note_type,
            tags,
            extra_frontmatter,
            body,
            observations,
            relations,
            updated_at_millis: mtime,
        }
    }

    /// Renders `note` as file text. A blank body is generated from the observations and relations;
    /// a given body is kept, with observations/relations it lacks appended to their sections.
    pub fn render(note: &Note) -> String {
        let body = if note.body.trim().is_empty() {
            render_body(note)
        } else {
            merge_body(&note.body, &note.observations, &note.relations)
        };
        let mut out = String::from("---\n");
        out.push_str(&render_frontmatter(note));
        out.push_str("---\n");
        out.push_str(&body);
        if !body.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// Parses one `- [category] content #tag (context)` line, or returns `None`.
    pub fn parse_observation(line: &str) -> Option<Observation> {
        let captures = OBSERVATION.captures(line.trim())?;
        let category = captures.get(1)?.as_str().trim();
        // Task-list checkboxes `- [ ]` / `- [x]` are not observations.
        if category.is_empty() || category.eq_ignore_ascii_case("x") {
            return None;
        }
        let mut rest = captures.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        let mut context = None;
        if rest.ends_with(')') {
            let open = matching_open_paren(rest);
            if open > 0
                && rest[..open]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace)
            {
                let inner = rest[open + 1..rest.len() - 1].trim();
                context = (!inner.is_empty()).then(|| inner.to_string());
                rest = rest[..open].trim();
            }
        }
        let tags = tag_spans(rest).into_iter().map(|span| span.tag).collect();
        let without_tags = remove_tags(rest);
        let content = SPACES.replace_all(&without_tags, " ").trim().to_string();
        Some(Observation {
            category: category.to_string(),
            content,
            tags,
            context,
        })
    }

    /// Parses one `- relation_type [[Target]] (context)` line, or returns `None`.
    pub fn parse_relation(line: &str) -> Option<Relation> {
        let captures = RELATION.captures(line.trim())?;
        let target = link_target(captures.get(2)?.as_str())?;
        Some(Relation {
            relation_type: captures.get(1)?.as_str().trim().to_string(),
            target,
            context: captures
                .get(3)
                .map(|m| m.as_str().trim())
                .filter(|context| !context.is_empty())
                .map(str::to_string),
        })
    }

    pub fn render_observation(observation: &Observation) -> String {
        let mut out = String::new();
        out.push_str("- [");
        out.push_str(
            &one_line(&observation.category)
                .replace('[', "(")
                .replace(']', ")"),
        );
        out.push_str("] ");
        out.push_str(&one_line(&observation.content));
        for tag in &observation.tags {
            let tag = tag.trim();
            let tag = tag.strip_prefix('#').unwrap_or(tag);
            let tag = SPACES.replace_all(tag, "_");
            if !tag.is_empty() {
                out.push_str(" #");
                out.push_str(&tag);
            }
        }
        if let Some(context) = &observation.context {
            let context = one_line(context);
            if !context.is_empty() {
                out.push_str(&format!(" ({context})"));
            }
        }
        out.trim_end().to_string()
    }

    pub fn render_relation(relation: &Relation) -> String {
        let relation_type = one_line(&relation.relation_type);
        let relation_type = if relation_type.is_empty() {
            LINKS_TO
        } else {
            &relation_type
        };
        let mut out = format!("- {relation_type} [[{}]]", one_line(&relation.target));
        if let Some(context) = &relation.context {
            let context = one_line(context);
            if !context.is_empty() {
                out.push_str(&format!(" ({context})"));
            }
        }
        out
    }
}

/// Observations and relations (explicit plus `links_to`) found in `body`.
pub fn parse_body(body: &str) -> (Vec<Observation>, Vec<Relation>) {
    let mut observations = Vec::new();
    let mut relations = Vec::new();
    let mut links = Vec::new();
    for_each_content_line(body, |line| {
        if let Some(observation) = BasicMemoryFormat::parse_observation(line) {
            observations.push(observation);
            collect_links(line, &mut links);
            return;
        }
        match BasicMemoryFormat::parse_relation(line) {
            Some(relation) => relations.push(relation),
            None => collect_links(line, &mut links),
        }
    });
    relations.extend(links);
    (observations, distinct(relations))
}

fn render_body(note: &Note) -> String {
    let mut out = format!("# {}\n", one_line(&note.title));
    if !note.observations.is_empty() {
        out.push_str("\n## Observations\n");
        for observation in &note.observations {
            out.push_str(&BasicMemoryFormat::render_observation(observation));
            out.push('\n');
        }
    }
    if !note.relations.is_empty() {
        out.push_str("\n## Relations\n");
        for relation in distinct(note.relations.clone()) {
            out.push_str(&BasicMemoryFormat::render_relation(&relation));
            out.push('\n');
        }
    }
    out
}

fn merge_body(body: &str, observations: &[Observation], relations: &[Relation]) -> String {
    let (present, present_relations) = parse_body(body);
    let mut missing = Vec::new();
    for observation in observations {
        let candidate = BasicMemoryFormat::parse_observation(
            &BasicMemoryFormat::render_observation(observation),
        )
        .unwrap_or_else(|| observation.clone());
        if !present.contains(&candidate) && !missing.contains(&candidate) {
            missing.push(candidate);
        }
    }
    let mut missing_relations = Vec::new();
    for relation in relations {
        let candidate =
            BasicMemoryFormat::parse_relation(&BasicMemoryFormat::render_relation(relation))
                .unwrap_or_else(|| relation.clone());
        if !present_relations.contains(&candidate) && !missing_relations.contains(&candidate) {
            missing_relations.push(candidate);
        }
    }
    if missing.is_empty() && missing_relations.is_empty() {
        return body.to_string();
    }
    let mut lines: Vec<String> = body.split('\n').map(str::to_string).collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if !missing.is_empty() {
        let new_lines = missing
            .iter()
            .map(BasicMemoryFormat::render_observation)
            .collect();
        insert_into_section(&mut lines, "Observations", new_lines, Some("Relations"));
    }
    if !missing_relations.is_empty() {
        let new_lines = missing_relations
            .iter()
            .map(BasicMemoryFormat::render_relation)
            .collect();
        insert_into_section(&mut lines, "Relations", new_lines, None);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn insert_into_section(
    lines: &mut Vec<String>,
    section: &str,
    new_lines: Vec<String>,
    before: Option<&str>,
) {
    if let Some(heading) = lines
        .iter()
        .position(|line| is_section_heading(line, section))
    {
        let mut end = heading + 1;
        while end < lines.len() && !SECTION_END.is_match(lines[end].trim_start()) {
            end += 1;
        }
        let mut at = end;
        while at > heading + 1 && lines[at - 1].trim().is_empty() {
            at -= 1;
        }
        for (offset, line) in new_lines.into_iter().enumerate() {
            lines.insert(at + offset, line);
        }
        return;
    }
    let mut block = vec![format!("## {section}")];
    block.extend(new_lines);
    let next = before.and_then(|name| lines.iter().position(|line| is_section_heading(line, name)));
    match next {
        Some(next) => {
            block.push(String::new());
            for (offset, line) in block.into_iter().enumerate() {
                lines.insert(next + offset, line);
            }
        }
        None => {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.extend(block);
        }
    }
}

fn is_section_heading(line: &str, name: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("## ")
        && trimmed
            .strip_prefix("##")
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case(name)
}

fn render_frontmatter(note: &Note) -> String {
    let mut out = String::new();
    out.push_str(&format!("title: {}\n", yaml_scalar(&note.title)));
    let note_type = if note.note_type.trim().is_empty() {
        "note"
    } else {
        note.note_type.as_str()
    };
    out.push_str(&format!("type: {}\n", yaml_scalar(note_type)));
    out.push_str(&format!("permalink: {}\n", yaml_scalar(&note.permalink)));
    let tags: Vec<String> = note.tags.iter().map(|tag| yaml_scalar(tag)).collect();
    out.push_str(&format!("tags: [{}]\n", tags.join(", ")));
    for (key, value) in &note.extra_frontmatter {
        if !RESERVED.contains(&key.as_str()) {
            out.push_str(&yaml_entry(key, value, 0));
        }
    }
    out
}

/// One frontmatter entry, block style for maps and flow style for everything else.
///
/// This writer and the Kotlin `renderFrontmatter` are the two halves of one contract: the golden
/// fixtures under `fixtures/memory/` pin their byte-for-byte agreement.
fn yaml_entry(key: &str, value: &Value, indent: usize) -> String {
    let pad = " ".repeat(indent);
    match value {
        Value::Object(map) if !map.is_empty() => {
            let mut out = format!("{pad}{}:\n", yaml_scalar(key));
            for (nested, nested_value) in map {
                out.push_str(&yaml_entry(nested, nested_value, indent + 2));
            }
            out
        }
        _ => format!("{pad}{}: {}\n", yaml_scalar(key), yaml_flow(value)),
    }
}

fn yaml_flow(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                value.to_string()
            } else if let Some(value) = number.as_u64() {
                value.to_string()
            } else {
                yaml_double(number.as_f64().unwrap_or_default())
            }
        }
        Value::String(text) => yaml_scalar(text),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(yaml_flow).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Object(map) => {
            let items: Vec<String> = map
                .iter()
                .map(|(key, value)| format!("{}: {}", yaml_scalar(key), yaml_flow(value)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

/// `Double.toString` for ordinary values: an integral value keeps `.0`.
fn yaml_double(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e16 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

fn yaml_scalar(text: &str) -> String {
    let keywords = [
        "true", "false", "null", "yes", "no", "on", "off", "~", "nan", "inf", "infinity",
    ];
    let unsafe_start = matches!(
        text.chars().next(),
        Some(
            ' ' | '\t'
                | '-'
                | '?'
                | ':'
                | ','
                | '['
                | ']'
                | '{'
                | '}'
                | '#'
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '\''
                | '"'
                | '%'
                | '@'
                | '`'
        )
    );
    let needs_quotes = text.is_empty()
        || text.trim() != text
        || unsafe_start
        || text.contains(": ")
        || text.contains(" #")
        || text.contains(['\n', '\r', '\t'])
        || keywords.contains(&text.to_lowercase().as_str())
        || text.parse::<f64>().is_ok()
        || text.parse::<i64>().is_ok();
    if !needs_quotes {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn yaml_to_json(value: serde_yaml_ng::Value) -> Value {
    match value {
        serde_yaml_ng::Value::Null => Value::Null,
        serde_yaml_ng::Value::Bool(value) => Value::Bool(value),
        serde_yaml_ng::Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                Value::from(value)
            } else if let Some(value) = number.as_u64() {
                Value::from(value)
            } else {
                Value::from(number.as_f64().unwrap_or_default())
            }
        }
        serde_yaml_ng::Value::String(text) => Value::String(text),
        serde_yaml_ng::Value::Sequence(items) => {
            Value::Array(items.into_iter().map(yaml_to_json).collect())
        }
        serde_yaml_ng::Value::Mapping(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                out.insert(yaml_key(&key), yaml_to_json(value));
            }
            Value::Object(out)
        }
        serde_yaml_ng::Value::Tagged(tagged) => yaml_to_json(tagged.value),
    }
}

fn yaml_key(value: &serde_yaml_ng::Value) -> String {
    match value {
        serde_yaml_ng::Value::String(text) => text.clone(),
        serde_yaml_ng::Value::Bool(value) => value.to_string(),
        serde_yaml_ng::Value::Number(number) => number.to_string(),
        other => serde_yaml_ng::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

fn split_frontmatter(text: &str) -> (Map<String, Value>, String) {
    let normalized = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    if lines.first().map(|line| line.trim_end()) != Some("---") {
        return (Map::new(), normalized);
    }
    let close = (1..lines.len()).find(|&index| matches!(lines[index].trim_end(), "---" | "..."));
    let Some(close) = close else {
        return (Map::new(), normalized);
    };
    let yaml = lines[1..close].join("\n");
    let body = lines[close + 1..].join("\n");
    if yaml.trim().is_empty() {
        return (Map::new(), body);
    }
    let Ok(loaded) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&yaml) else {
        // Unreadable frontmatter: keep the whole text as body so nothing is lost on rewrite.
        return (Map::new(), normalized);
    };
    let serde_yaml_ng::Value::Mapping(map) = loaded else {
        return (Map::new(), body);
    };
    let mut out = Map::new();
    for (key, value) in map {
        out.insert(yaml_key(&key), yaml_to_json(value));
    }
    (out, body)
}

fn parse_tags(value: Option<&Value>) -> Vec<String> {
    let raw: Vec<String> = match value {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(json_text)
            .map(|text| text.unwrap_or_default())
            .collect(),
        Some(other) => json_text(other)
            .unwrap_or_default()
            .split(',')
            .map(str::to_string)
            .collect(),
    };
    raw.iter()
        .map(|tag| tag.trim())
        .map(|tag| tag.strip_prefix('#').unwrap_or(tag))
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

fn json_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        other => Some(other.to_string()),
    }
}

fn first_heading(body: &str) -> Option<String> {
    let mut heading = None;
    for_each_content_line(body, |line| {
        if heading.is_none() && line.starts_with("# ") {
            let text = line.trim_start_matches("# ").trim();
            if !text.is_empty() {
                heading = Some(text.to_string());
            }
        }
    });
    heading
}

/// Calls `action` with each trimmed line outside fenced code blocks.
fn for_each_content_line(body: &str, mut action: impl FnMut(&str)) {
    let mut fence: Option<String> = None;
    for raw in body.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            let marker: String = line.chars().take(3).collect();
            fence = match &fence {
                None => Some(marker),
                Some(current) if *current == marker => None,
                Some(current) => Some(current.clone()),
            };
            continue;
        }
        if fence.is_none() {
            action(line);
        }
    }
}

fn collect_links(line: &str, into: &mut Vec<Relation>) {
    let without_code = INLINE_CODE.replace_all(line, "");
    for captures in WIKI_LINK.captures_iter(&without_code) {
        if let Some(target) = captures.get(1).and_then(|m| link_target(m.as_str())) {
            into.push(Relation {
                relation_type: LINKS_TO.to_string(),
                target,
                context: None,
            });
        }
    }
}

/// `Target|alias` and `Target#heading` both point at `Target`.
fn link_target(raw: &str) -> Option<String> {
    let target = raw.split('|').next().unwrap_or(raw);
    let target = target.split('#').next().unwrap_or(target).trim();
    (!target.is_empty()).then(|| target.to_string())
}

fn matching_open_paren(text: &str) -> usize {
    let mut depth = 0i32;
    for (index, c) in text.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    return index;
                }
            }
            _ => {}
        }
    }
    usize::MAX
}

struct TagSpan {
    start: usize,
    end: usize,
    tag: String,
}

fn tag_spans(text: &str) -> Vec<TagSpan> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut spans = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let (start, c) = chars[index];
        let preceded_by_space = index == 0 || chars[index - 1].1.is_whitespace();
        if c == '#' && preceded_by_space {
            let mut end_index = index + 1;
            while end_index < chars.len() {
                let candidate = chars[end_index].1;
                if candidate.is_whitespace() || matches!(candidate, '#' | '(' | ')' | '[' | ']') {
                    break;
                }
                end_index += 1;
            }
            if end_index > index + 1 {
                let end = chars
                    .get(end_index)
                    .map(|(offset, _)| *offset)
                    .unwrap_or(text.len());
                spans.push(TagSpan {
                    start,
                    end,
                    tag: text[start + 1..end].to_string(),
                });
                index = end_index;
                continue;
            }
        }
        index += 1;
    }
    spans
}

fn remove_tags(text: &str) -> String {
    let spans = tag_spans(text);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for span in spans {
        out.push_str(&text[cursor..span.start]);
        cursor = span.end;
    }
    out.push_str(&text[cursor..]);
    out
}

fn one_line(text: &str) -> String {
    SPACES.replace_all(text, " ").trim().to_string()
}

fn distinct<T: PartialEq>(items: Vec<T>) -> Vec<T> {
    let mut out = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}
