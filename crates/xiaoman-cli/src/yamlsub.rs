//! YAML-subset view of `SKILL.md` frontmatter and `flow.yaml`.
//!
//! The YAML grammar and line numbers come from [`xiaoman_skills::yaml`] (the parser the app
//! uses); this module adds the value semantics the CLI needs on top of it: typed plain scalars,
//! the scalar's raw text, `key_line`, and duplicate-key reporting. It also rejects the
//! constructs the CLI's original YAML subset did not support (anchors, aliases, tags), so a
//! skill that lints clean here keeps linting clean in the app.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};
use xiaoman_skills::yaml::{Node as YamlNode, YamlDoc};

static INT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-+]?[0-9]+$").unwrap());
static FLOAT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$").unwrap());

/// A parsed YAML value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Node>),
    Map(Vec<(String, Node)>),
}

/// A value plus the 1-based line it starts on.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub value: Value,
    /// Line the value starts on (file line, `line_offset` included).
    pub line: usize,
    /// Scalar text before type resolution; `None` for null scalars and collections.
    pub raw: Option<String>,
    /// Line of the mapping key when this node is a mapping value.
    pub key_line: usize,
}

impl Node {
    pub fn is_scalar(&self) -> bool {
        matches!(
            self.value,
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Str(_)
        )
    }

    pub fn is_null(&self) -> bool {
        matches!(self.value, Value::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }

    /// Mapping entries in source order; duplicate keys keep their first value.
    pub fn entries(&self) -> Option<&[(String, Node)]> {
        match &self.value {
            Value::Map(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn items(&self) -> Option<&[Node]> {
        match &self.value {
            Value::List(items) => Some(items),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entries()?
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, node)| node)
    }
}

/// A YAML syntax error: a message and the 1-based line it applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlError {
    pub message: String,
    pub line: usize,
}

impl std::fmt::Display for YamlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for YamlError {}

/// A missing or unclosed `---` frontmatter block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmatterError {
    pub message: String,
    pub line: usize,
}

impl std::fmt::Display for FrontmatterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for FrontmatterError {}

/// Parses a document with no line offset and duplicate keys rejected.
pub fn parse(text: &str) -> Result<Node, YamlError> {
    parse_with(text, 0, None)
}

/// Parses a document; `line_offset` is added to every reported line.
///
/// Duplicate mapping keys raise [`YamlError`] unless `duplicates` is given: the
/// `(key, line)` pairs are appended to it and the first value is kept.
pub fn parse_with(
    text: &str,
    line_offset: usize,
    mut duplicates: Option<&mut Vec<(String, usize)>>,
) -> Result<Node, YamlError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    reject_unsupported(text, line_offset)?;
    let mut doc = YamlDoc::new("yaml", line_offset);
    let root = doc.compose(text);
    if let Some(problem) = doc.problems.first() {
        let message = problem
            .message
            .strip_prefix("invalid YAML: ")
            .unwrap_or(&problem.message)
            .to_string();
        let line = clamp_error_line(problem.line.unwrap_or(1 + line_offset), text, line_offset);
        return Err(YamlError { message, line });
    }
    let Some(root) = root else {
        return Ok(Node {
            value: Value::Null,
            line: 1 + line_offset,
            raw: None,
            key_line: 1 + line_offset,
        });
    };
    let mut error = None;
    let node = convert(&root, line_offset, &mut duplicates, &mut error);
    match error {
        Some(error) => Err(error),
        None => Ok(node),
    }
}

/// Unwraps the nodes into plain JSON values, like the app's `to_plain`.
pub fn to_plain(node: &Node) -> JsonValue {
    match &node.value {
        Value::Null => JsonValue::Null,
        Value::Bool(value) => JsonValue::Bool(*value),
        Value::Int(value) => JsonValue::Number((*value).into()),
        Value::Float(value) => {
            JsonNumber::from_f64(*value).map_or(JsonValue::Null, JsonValue::Number)
        }
        Value::Str(text) => JsonValue::String(text.clone()),
        Value::List(items) => JsonValue::Array(items.iter().map(to_plain).collect()),
        Value::Map(entries) => {
            let mut map: JsonMap<String, JsonValue> = JsonMap::new();
            for (key, value) in entries {
                map.insert(key.clone(), to_plain(value));
            }
            JsonValue::Object(map)
        }
    }
}

/// Splits `---`-delimited frontmatter from a Markdown file.
///
/// Returns `(yaml_text, yaml_first_line, body)`; `yaml_first_line` is always 2 because the
/// opening `---` is line 1.
pub fn split_frontmatter(text: &str) -> Result<(String, usize, String), FrontmatterError> {
    match xiaoman_skills::split_frontmatter(text) {
        Some((yaml, body)) => Ok((yaml, 2, body)),
        None => {
            let normalized = text.strip_prefix('\u{feff}').unwrap_or(text);
            let first = normalized.lines().next().map(str::trim_end);
            let message = if first == Some("---") {
                "frontmatter is not closed with '---'"
            } else {
                "SKILL.md must start with a '---' frontmatter line"
            };
            Err(FrontmatterError {
                message: message.to_string(),
                line: 1,
            })
        }
    }
}

fn convert(
    node: &YamlNode,
    offset: usize,
    duplicates: &mut Option<&mut Vec<(String, usize)>>,
    error: &mut Option<YamlError>,
) -> Node {
    match node {
        YamlNode::Scalar { value, plain, line } => {
            let line = line + offset;
            if *plain {
                let resolved = resolve_plain(value);
                let raw = if matches!(resolved, Value::Null) {
                    None
                } else {
                    Some(value.clone())
                };
                Node {
                    value: resolved,
                    line,
                    raw,
                    key_line: line,
                }
            } else {
                Node {
                    value: Value::Str(value.clone()),
                    line,
                    raw: Some(value.clone()),
                    key_line: line,
                }
            }
        }
        YamlNode::Sequence { items, line } => {
            let items = items
                .iter()
                .map(|item| convert(item, offset, duplicates, error))
                .collect();
            Node {
                value: Value::List(items),
                line: line + offset,
                raw: None,
                key_line: line + offset,
            }
        }
        YamlNode::Mapping { entries, line } => {
            let mut out: Vec<(String, Node)> = Vec::new();
            for (key_node, value_node) in entries {
                let key_line = key_node.line() + offset;
                let key = match key_node.scalar() {
                    Some(key) => key.to_string(),
                    None => {
                        if error.is_none() {
                            *error = Some(YamlError {
                                message: "complex keys are not supported".into(),
                                line: key_line,
                            });
                        }
                        String::new()
                    }
                };
                let value = convert(value_node, offset, duplicates, error);
                if out.iter().any(|(seen, _)| *seen == key) {
                    match duplicates {
                        Some(list) => list.push((key, key_line)),
                        None => {
                            if error.is_none() {
                                *error = Some(YamlError {
                                    message: format!("duplicate key '{key}'"),
                                    line: key_line,
                                });
                            }
                        }
                    }
                } else {
                    out.push((key, Node { key_line, ..value }));
                }
            }
            Node {
                value: Value::Map(out),
                line: line + offset,
                raw: None,
                key_line: line + offset,
            }
        }
    }
}

fn resolve_plain(text: &str) -> Value {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        "true" | "True" | "TRUE" => Value::Bool(true),
        "false" | "False" | "FALSE" => Value::Bool(false),
        _ => {
            if INT_RE.is_match(text) {
                if let Ok(number) = text.parse::<i64>() {
                    return Value::Int(number);
                }
            }
            if FLOAT_RE.is_match(text) && text.bytes().any(|byte| byte.is_ascii_digit()) {
                if let Ok(number) = text.parse::<f64>() {
                    return Value::Float(number);
                }
            }
            Value::Str(text.to_string())
        }
    }
}

/// yaml-rust2 points past the end of the file for unterminated flow collections; the CLI
/// reports the last line that has content, like its original parser did.
fn clamp_error_line(reported: usize, text: &str, offset: usize) -> usize {
    let last = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, _)| index + 1 + offset)
        .last()
        .unwrap_or(1 + offset);
    reported.min(last.max(1 + offset))
}

/// Rejects the constructs the CLI's YAML subset did not support, at the line they start on.
fn reject_unsupported(text: &str, offset: usize) -> Result<(), YamlError> {
    let mut block_scalar_indent: Option<usize> = None;
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1 + offset;
        let indent = raw.len() - raw.trim_start_matches(' ').len();
        if let Some(header_indent) = block_scalar_indent {
            if raw.trim().is_empty() {
                continue;
            }
            if indent > header_indent {
                continue;
            }
            block_scalar_indent = None;
        }
        let content = raw.trim_start();
        if content.is_empty() {
            continue;
        }
        if let Some(value) = value_after_marker(content) {
            let value = value.trim_start();
            if value.starts_with('|') || value.starts_with('>') {
                block_scalar_indent = Some(indent);
                continue;
            }
            if value.starts_with('&') || value.starts_with('*') || value.starts_with('!') {
                return Err(YamlError {
                    message: "anchors, aliases and tags are not supported".into(),
                    line,
                });
            }
        } else if indent == 0 {
            let first = content.chars().next().unwrap_or_default();
            if matches!(first, '&' | '*' | '!') {
                return Err(YamlError {
                    message: "anchors, aliases and tags are not supported".into(),
                    line,
                });
            }
        }
    }
    Ok(())
}

/// The value text of a mapping entry or sequence item on this line, if it starts one.
fn value_after_marker(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("- ") {
        let rest = rest.trim_start();
        if rest.is_empty() {
            return None;
        }
        return Some(value_after_marker(rest).unwrap_or(rest));
    }
    if line == "-" {
        return None;
    }
    let colon = key_colon(line)?;
    Some(&line[colon + 1..])
}

/// Index of the `:` that separates a mapping key from its value, if there is one.
fn key_colon(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut quote = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(open) = quote {
            if open == b'"' && byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == open {
                quote = None;
            }
        } else if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
        } else if byte == b':'
            && (index + 1 == bytes.len() || bytes[index + 1] == b' ' || bytes[index + 1] == b'\t')
        {
            return Some(index);
        } else if byte == b' ' && index + 1 < bytes.len() && bytes[index + 1] == b'#' {
            return None;
        }
        index += 1;
    }
    None
}
