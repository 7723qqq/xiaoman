//! Node-level YAML access that keeps line numbers.
//!
//! The tree is built from `yaml-rust2` parser events so every node carries the 1-indexed source line
//! reported by the scanner. `line_offset` is the number of file lines before the YAML text (the
//! opening `---` of a frontmatter block), so reported lines are 1-based file lines.

use std::sync::LazyLock;

use regex::Regex;
use xiaoman_api::SkillProblem;
use yaml_rust2::parser::{Event, Parser};
use yaml_rust2::scanner::{Marker, TScalarStyle};

/// Android application id: two or more dot-separated Java identifiers.
pub static PACKAGE_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z][a-zA-Z0-9_]*(\.[a-zA-Z][a-zA-Z0-9_]*)+$").unwrap());

/// `scheme://rest` with a URI-style scheme; `{placeholders}` are allowed in the rest.
pub static DEEPLINK_FORM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.-]*://\S+$").unwrap());

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Scalar {
        value: String,
        plain: bool,
        line: usize,
    },
    Sequence {
        items: Vec<Node>,
        line: usize,
    },
    Mapping {
        entries: Vec<(Node, Node)>,
        line: usize,
    },
}

impl Node {
    /// 1-indexed line inside the YAML text the node was parsed from.
    pub fn line(&self) -> usize {
        match self {
            Self::Scalar { line, .. }
            | Self::Sequence { line, .. }
            | Self::Mapping { line, .. } => *line,
        }
    }

    pub fn is_null(&self) -> bool {
        match self {
            Self::Scalar { value, plain, .. } => {
                *plain
                    && (value.is_empty()
                        || matches!(value.as_str(), "~" | "null" | "Null" | "NULL"))
            }
            _ => false,
        }
    }

    pub fn scalar(&self) -> Option<&str> {
        match self {
            Self::Scalar { value, .. } => Some(value),
            _ => None,
        }
    }

    pub fn sequence(&self) -> Option<&[Node]> {
        match self {
            Self::Sequence { items, .. } => Some(items),
            _ => None,
        }
    }

    pub fn mapping(&self) -> Option<&[(Node, Node)]> {
        match self {
            Self::Mapping { entries, .. } => Some(entries),
            _ => None,
        }
    }
}

pub struct Entry {
    pub key: String,
    pub key_line: usize,
    pub value: Node,
}

/// YAML text plus the file line it starts on, so reported lines are 1-based file lines.
pub struct YamlDoc {
    source: String,
    line_offset: usize,
    pub problems: Vec<SkillProblem>,
}

impl YamlDoc {
    pub fn new(source: &str, line_offset: usize) -> Self {
        Self {
            source: source.to_string(),
            line_offset,
            problems: Vec::new(),
        }
    }

    pub fn problem(&mut self, message: impl Into<String>, node: &Node) {
        let line = self.line_of(node);
        self.problem_at(message, Some(line));
    }

    pub fn problem_at(&mut self, message: impl Into<String>, line: Option<usize>) {
        self.problems.push(SkillProblem {
            source: self.source.clone(),
            message: message.into(),
            line,
        });
    }

    pub fn line_of(&self, node: &Node) -> usize {
        node.line() + self.line_offset
    }

    /// Composes `text` into a node tree; `None` with a problem on a syntax error, `None` alone when empty.
    pub fn compose(&mut self, text: &str) -> Option<Node> {
        match parse_tree(text) {
            Ok(node) => node,
            Err((message, line)) => {
                self.problem_at(
                    format!("invalid YAML: {message}"),
                    line.map(|line| line + self.line_offset),
                );
                None
            }
        }
    }

    /// Mapping entries by key; non-scalar or repeated keys are reported.
    pub fn entries(&mut self, mapping: &Node) -> Vec<Entry> {
        let mut out: Vec<Entry> = Vec::new();
        let Some(entries) = mapping.mapping() else {
            return out;
        };
        for (key_node, value) in entries {
            let Some(key) = key_node.scalar() else {
                self.problem("mapping keys must be plain strings", key_node);
                continue;
            };
            if out.iter().any(|entry| entry.key == key) {
                let line = self.line_of(key_node);
                self.problem_at(format!("duplicate key '{key}'"), Some(line));
                continue;
            }
            out.push(Entry {
                key: key.to_string(),
                key_line: self.line_of(key_node),
                value: value.clone(),
            });
        }
        out
    }

    /// A non-null scalar as a string, or `None` with a problem.
    pub fn string(&mut self, entry: &Entry) -> Option<String> {
        if let Node::Scalar { value, .. } = &entry.value {
            if !entry.value.is_null() {
                return Some(value.clone());
            }
        }
        self.problem_at(
            format!("'{}' must be a string", entry.key),
            Some(entry.key_line),
        );
        None
    }

    /// A list of strings; a single string is accepted as a one-element list, `null` as empty.
    pub fn string_list(&mut self, entry: &Entry) -> Option<Vec<String>> {
        match &entry.value {
            Node::Scalar { .. } if entry.value.is_null() => Some(Vec::new()),
            Node::Scalar { value, .. } => Some(vec![value.clone()]),
            Node::Sequence { items, .. } => {
                let mut out = Vec::new();
                for item in items {
                    match item {
                        Node::Scalar { value, .. } if !item.is_null() => {
                            out.push(value.trim().to_string())
                        }
                        _ => {
                            self.problem(format!("'{}' items must be strings", entry.key), item);
                            return None;
                        }
                    }
                }
                if out.iter().any(|item| item.is_empty()) {
                    self.problem_at(
                        format!("'{}' items must not be empty", entry.key),
                        Some(entry.key_line),
                    );
                    return None;
                }
                Some(out)
            }
            _ => {
                self.problem_at(
                    format!("'{}' must be a list of strings", entry.key),
                    Some(entry.key_line),
                );
                None
            }
        }
    }
}

fn parse_tree(text: &str) -> Result<Option<Node>, (String, Option<usize>)> {
    let mut parser = Parser::new_from_str(text);
    let mut events: Vec<(Event, Marker)> = Vec::new();
    loop {
        match parser.next_token() {
            Ok((Event::StreamEnd, _)) => break,
            Ok(pair) => events.push(pair),
            Err(error) => {
                let line = Some(error.marker().line());
                return Err((error.info().to_string(), line));
            }
        }
    }
    let mut index = 0;
    while index < events.len()
        && matches!(events[index].0, Event::StreamStart | Event::DocumentStart)
    {
        index += 1;
    }
    if index >= events.len() || matches!(events[index].0, Event::DocumentEnd | Event::StreamEnd) {
        return Ok(None);
    }
    Ok(build(&events, &mut index))
}

fn build(events: &[(Event, Marker)], index: &mut usize) -> Option<Node> {
    let (event, marker) = events.get(*index)?;
    let line = marker.line();
    match event {
        Event::Scalar(value, style, _, _) => {
            *index += 1;
            Some(Node::Scalar {
                value: value.clone(),
                plain: *style == TScalarStyle::Plain,
                line,
            })
        }
        Event::SequenceStart(_, _) => {
            *index += 1;
            let mut items = Vec::new();
            while *index < events.len() && !matches!(events[*index].0, Event::SequenceEnd) {
                match build(events, index) {
                    Some(node) => items.push(node),
                    None => break,
                }
            }
            if *index < events.len() {
                *index += 1;
            }
            Some(Node::Sequence { items, line })
        }
        Event::MappingStart(_, _) => {
            *index += 1;
            let mut entries = Vec::new();
            while *index < events.len() && !matches!(events[*index].0, Event::MappingEnd) {
                let key = build(events, index)?;
                let value = build(events, index)?;
                entries.push((key, value));
            }
            if *index < events.len() {
                *index += 1;
            }
            Some(Node::Mapping { entries, line })
        }
        _ => {
            *index += 1;
            None
        }
    }
}
