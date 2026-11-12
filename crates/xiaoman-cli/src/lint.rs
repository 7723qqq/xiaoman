//! Skill folder validation with the same rules, messages and line numbers as the app.
//!
//! The YAML layer is [`crate::yamlsub`], which parses through `xiaoman-skills`; the checks
//! mirror the app's `SkillParser`/`FlowParser` and the shared constants (`KEYS`, channel
//! names, step kinds, wait limit, package/deeplink patterns) come from those crates.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use xiaoman_api::SkillChannel;
use xiaoman_skills::yaml::{DEEPLINK_FORM, PACKAGE_NAME};
use xiaoman_skills::{FlowParser, SkillParser};

use crate::yamlsub::{self, Node, Value};

static LONG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+-]?[0-9]+$").unwrap());

/// Channel names accepted in `SKILL.md`, in the order the error message lists them.
pub const CHANNELS: [&str; 7] = [
    "api",
    "cli",
    "deeplink",
    "gui",
    "deeplink-then-gui",
    "mixed",
    "chat",
];

/// One lint problem: a file, the 1-based line and the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: String,
    pub line: usize,
    pub message: String,
}

impl Problem {
    pub fn new(path: impl Into<String>, line: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            line: line.unwrap_or(1),
            message: message.into(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.path, self.line, self.message)
    }
}

/// Lint result of one folder; `name` is set when `SKILL.md` is valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInfo {
    pub directory: PathBuf,
    pub name: Option<String>,
    pub problems: Vec<Problem>,
}

impl SkillInfo {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// What a valid `SKILL.md` says about the skill, for the flow checks and for `push`.
struct SkillFacts {
    name: String,
    apps: Vec<String>,
    deeplinks: Vec<(String, String)>,
    channel: String,
}

struct Doc<'a> {
    path: String,
    problems: &'a mut Vec<Problem>,
}

impl Doc<'_> {
    fn problem(&mut self, message: impl Into<String>, line: Option<usize>) {
        self.problems
            .push(Problem::new(self.path.clone(), line, message));
    }

    /// Parses `text`; returns the root and whether parsing succeeded.
    fn compose(&mut self, text: &str, line_offset: usize) -> (Option<Node>, bool) {
        let mut duplicates = Vec::new();
        match yamlsub::parse_with(text, line_offset, Some(&mut duplicates)) {
            Ok(root) => {
                for (key, line) in duplicates {
                    self.problem(format!("duplicate key '{key}'"), Some(line));
                }
                let absent = root.is_null() && root.raw.is_none();
                (if absent { None } else { Some(root) }, true)
            }
            Err(error) => {
                self.problem(format!("invalid YAML: {}", error.message), Some(error.line));
                (None, false)
            }
        }
    }

    /// A non-null scalar's text, or `None` with a problem.
    fn string(&mut self, key: &str, node: &Node) -> Option<String> {
        if node.is_scalar() && node.raw.is_some() {
            return node.raw.clone();
        }
        self.problem(format!("'{key}' must be a string"), Some(node.key_line));
        None
    }

    /// List of strings; a single string is a one-element list, null is empty.
    fn string_list(&mut self, key: &str, node: &Node) -> Option<Vec<String>> {
        if node.is_scalar() {
            return Some(node.raw.clone().into_iter().collect());
        }
        let Some(items) = node.items() else {
            self.problem(
                format!("'{key}' must be a list of strings"),
                Some(node.key_line),
            );
            return None;
        };
        let mut result = Vec::new();
        for item in items {
            if !item.is_scalar() || item.raw.is_none() {
                self.problem(format!("'{key}' items must be strings"), Some(item.line));
                return None;
            }
            result.push(item.raw.clone().unwrap_or_default().trim().to_string());
        }
        if result.iter().any(|item| item.is_empty()) {
            self.problem(
                format!("'{key}' items must not be empty"),
                Some(node.key_line),
            );
            return None;
        }
        Some(result)
    }
}

/// Lints one skill folder.
pub fn lint_skill(directory: &Path) -> SkillInfo {
    let mut info = SkillInfo {
        directory: directory.to_path_buf(),
        name: None,
        problems: Vec::new(),
    };
    if !directory.is_dir() {
        info.problems.push(Problem::new(
            directory.display().to_string(),
            None,
            "not a directory",
        ));
        return info;
    }
    let folder = folder_name(directory);
    let skill_md = directory.join("SKILL.md");
    if !skill_md.is_file() {
        info.problems.push(Problem::new(
            directory.display().to_string(),
            None,
            "missing SKILL.md",
        ));
        return info;
    }
    let skill_md_path = skill_md.display().to_string();
    let text = match std::fs::read_to_string(&skill_md) {
        Ok(text) => text,
        Err(error) => {
            info.problems.push(Problem::new(
                skill_md_path,
                None,
                format!("cannot read: {error}"),
            ));
            return info;
        }
    };
    let Some(skill) = lint_skill_md(&skill_md_path, &text, &mut info.problems) else {
        return info;
    };
    info.name = Some(skill.name.clone());
    if skill.name != folder {
        info.problems.push(Problem::new(
            skill_md_path,
            name_line(&text),
            format!(
                "name '{}' does not match folder name '{}'",
                skill.name, folder
            ),
        ));
    }
    let flow_yaml = directory.join("flow.yaml");
    if flow_yaml.is_file() {
        let flow_path = flow_yaml.display().to_string();
        if skill.channel == "chat" {
            info.problems.push(Problem::new(
                flow_path,
                Some(1),
                "聊天工作流不操作手机，不能设置 flow.yaml",
            ));
            return info;
        }
        match std::fs::read_to_string(&flow_yaml) {
            Ok(flow_text) => lint_flow_into(
                &flow_path,
                &flow_text,
                &skill.deeplinks,
                &skill.apps,
                &mut info.problems,
            ),
            Err(error) => {
                info.problems.push(Problem::new(
                    flow_path,
                    None,
                    format!("cannot read: {error}"),
                ));
            }
        }
    }
    info
}

/// Lints several skill folders.
pub fn lint_paths(directories: &[PathBuf]) -> Vec<SkillInfo> {
    directories
        .iter()
        .map(|directory| lint_skill(directory))
        .collect()
}

fn folder_name(directory: &Path) -> String {
    std::fs::canonicalize(directory)
        .unwrap_or_else(|_| directory.to_path_buf())
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Line of the `name:` key, or `None` when the file has none.
fn name_line(text: &str) -> Option<usize> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.split('\n')
        .position(|line| line.starts_with("name:"))
        .map(|index| index + 1)
}

fn lint_skill_md(path: &str, text: &str, problems: &mut Vec<Problem>) -> Option<SkillFacts> {
    let start = problems.len();
    let mut doc = Doc {
        path: path.to_string(),
        problems,
    };
    let Ok((yaml, first_line, _body)) = yamlsub::split_frontmatter(text) else {
        doc.problem(
            "missing frontmatter: SKILL.md must start with a '---' line and close it with another '---'",
            Some(1),
        );
        return None;
    };
    let (root, parsed) = doc.compose(&yaml, first_line.saturating_sub(1));
    if !parsed {
        return None;
    }
    if let Some(root) = &root {
        if root.entries().is_none() {
            doc.problem("frontmatter must be a mapping of keys", Some(root.line));
            return None;
        }
    }
    if doc.problems.len() > start {
        return None;
    }
    let entries: Vec<(String, Node)> = root
        .as_ref()
        .and_then(Node::entries)
        .map(<[(String, Node)]>::to_vec)
        .unwrap_or_default();
    let find = |key: &str| {
        entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, node)| node)
    };

    for (key, node) in &entries {
        if !SkillParser::KEYS.contains(&key.as_str()) {
            doc.problem(
                format!(
                    "unknown key '{key}'; expected one of {}",
                    SkillParser::KEYS.join(", ")
                ),
                Some(node.key_line),
            );
        }
    }

    let mut name = None;
    match find("name") {
        None => doc.problem("missing required key 'name'", Some(1)),
        Some(node) => {
            if let Some(raw) = doc.string("name", node) {
                let value = raw.trim().to_string();
                if !SkillParser::name_pattern().is_match(&value) {
                    doc.problem(
                        format!("name '{value}' must match [a-z0-9-]+"),
                        Some(node.key_line),
                    );
                }
                name = Some(value);
            }
        }
    }

    let mut description = None;
    match find("description") {
        None => doc.problem("missing required key 'description'", Some(1)),
        Some(node) => {
            if let Some(raw) = doc.string("description", node) {
                let value = raw.trim().to_string();
                if value.is_empty() {
                    doc.problem("'description' must not be empty", Some(node.key_line));
                } else if value.contains('\n') {
                    doc.problem("'description' must be a single line", Some(node.key_line));
                }
                description = Some(value);
            }
        }
    }

    let mut apps = Vec::new();
    if let Some(node) = find("apps") {
        if let Some(list) = doc.string_list("apps", node) {
            for app in &list {
                if !PACKAGE_NAME.is_match(app) {
                    doc.problem(
                        format!("apps: '{app}' is not a package name"),
                        Some(node.key_line),
                    );
                }
            }
            apps = list;
        }
    }

    let mut channel = "gui".to_string();
    if let Some(node) = find("channel") {
        if let Some(wire) = doc.string("channel", node) {
            if SkillChannel::from_wire(&wire).is_none() {
                doc.problem(
                    format!(
                        "unknown channel '{wire}'; expected one of {}",
                        CHANNELS.join(", ")
                    ),
                    Some(node.key_line),
                );
            }
            channel = wire.trim().to_lowercase();
        }
    }

    for key in ["confirm_before", "memory"] {
        if let Some(node) = find(key) {
            doc.string_list(key, node);
        }
    }

    let mut deeplinks = Vec::new();
    if let Some(node) = find("deeplinks") {
        deeplinks = lint_deeplinks(&mut doc, node);
    }

    if channel == "chat" {
        for key in ["apps", "deeplinks", "confirm_before"] {
            if let Some(node) = find(key) {
                doc.problem(
                    format!("聊天工作流不操作手机，不能设置 {key}"),
                    Some(node.key_line),
                );
            }
        }
    }

    let (Some(name), Some(_description)) = (name, description) else {
        return None;
    };
    if doc.problems.len() > start {
        return None;
    }
    Some(SkillFacts {
        name,
        apps,
        deeplinks,
        channel,
    })
}

fn brace_hint(nodes: &[&Node]) -> &'static str {
    if nodes.iter().any(|node| matches!(node.value, Value::Map(_))) {
        " (quote values that start with '{', e.g. \"{destination}\")"
    } else {
        ""
    }
}

fn lint_deeplinks(doc: &mut Doc, node: &Node) -> Vec<(String, String)> {
    if node.is_scalar() && node.raw.is_none() {
        return Vec::new();
    }
    let Some(entries) = node.entries() else {
        doc.problem(
            "'deeplinks' must be a mapping of name: \"scheme://...\"",
            Some(node.key_line),
        );
        return Vec::new();
    };
    let mut result = Vec::new();
    for (key, link) in entries {
        let value = if link.is_scalar() {
            link.raw.as_deref().map(|raw| raw.trim().to_string())
        } else {
            None
        };
        if key.trim().is_empty() {
            doc.problem("deeplinks: names must not be empty", Some(link.key_line));
        } else if value.is_none() {
            doc.problem(
                format!("deeplinks: '{key}' must be a string{}", brace_hint(&[link])),
                Some(link.key_line),
            );
        } else if !DEEPLINK_FORM.is_match(value.as_deref().unwrap_or_default()) {
            doc.problem(
                format!(
                    "deeplinks: '{key}' = '{}' is not a scheme:// URI",
                    value.unwrap_or_default()
                ),
                Some(link.key_line),
            );
        } else {
            result.push((key.clone(), value.unwrap_or_default()));
        }
    }
    result
}

/// Checks `flow.yaml` text; returns the problems found.
pub fn lint_flow(
    path: &str,
    text: &str,
    deeplinks: &[(String, String)],
    apps: &[String],
) -> Vec<Problem> {
    let mut problems = Vec::new();
    lint_flow_into(path, text, deeplinks, apps, &mut problems);
    problems
}

fn lint_flow_into(
    path: &str,
    text: &str,
    deeplinks: &[(String, String)],
    apps: &[String],
    problems: &mut Vec<Problem>,
) {
    let mut doc = Doc {
        path: path.to_string(),
        problems,
    };
    let (root, parsed) = doc.compose(text, 0);
    if !parsed {
        return;
    }
    let Some(root) = root else {
        doc.problem("flow.yaml is empty; expected 'steps:'", None);
        return;
    };
    if root.entries().is_none() {
        doc.problem("flow.yaml must be a mapping with 'steps:'", Some(root.line));
        return;
    }
    let entries: Vec<(String, Node)> = root.entries().unwrap_or(&[]).to_vec();
    for (key, node) in &entries {
        if key != "steps" {
            doc.problem(
                format!("unknown key '{key}'; only 'steps' is allowed"),
                Some(node.key_line),
            );
        }
    }
    let Some(steps) = entries
        .iter()
        .find(|(key, _)| key == "steps")
        .map(|(_, node)| node)
    else {
        doc.problem("missing 'steps:' list", Some(root.line));
        return;
    };
    let Some(items) = steps.items() else {
        doc.problem("'steps' must be a list", Some(steps.key_line));
        return;
    };
    if items.is_empty() {
        doc.problem("'steps' is empty", Some(steps.key_line));
    }
    let items = items.to_vec();
    for (index, step) in items.iter().enumerate() {
        lint_step(&mut doc, index + 1, step, deeplinks, apps);
    }
}

fn lint_step(
    doc: &mut Doc,
    number: usize,
    step: &Node,
    deeplinks: &[(String, String)],
    apps: &[String],
) {
    let entries = step.entries().unwrap_or(&[]);
    if entries.len() != 1 {
        doc.problem(
            format!("step {number}: expected one 'kind: value' pair, e.g. '- tap: 确认呼叫'"),
            Some(step.line),
        );
        return;
    }
    let (kind, node) = (&entries[0].0, &entries[0].1);
    let key_line = node.key_line;
    let text = || {
        if node.is_scalar() {
            node.raw
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        } else {
            None
        }
    };

    match kind.as_str() {
        "launch" => match text() {
            None => bad_step(
                doc,
                number,
                kind,
                key_line,
                "expected a package name".to_string(),
            ),
            Some(package) => {
                if !PACKAGE_NAME.is_match(&package) {
                    bad_step(
                        doc,
                        number,
                        kind,
                        key_line,
                        format!("'{package}' is not a package name"),
                    );
                } else if !apps.is_empty() && !apps.contains(&package) {
                    bad_step(
                        doc,
                        number,
                        kind,
                        key_line,
                        format!("'{package}' is not in the skill's apps"),
                    );
                }
            }
        },
        "deeplink" => match text() {
            None => bad_step(
                doc,
                number,
                kind,
                key_line,
                "expected a deeplinks key or a URI".to_string(),
            ),
            Some(value) => {
                if !deeplinks.iter().any(|(key, _)| key == &value)
                    && !DEEPLINK_FORM.is_match(&value)
                {
                    bad_step(
                        doc,
                        number,
                        kind,
                        key_line,
                        format!(
                            "'{value}' is neither a key of the skill's deeplinks nor a scheme:// URI"
                        ),
                    );
                }
            }
        },
        "ai" | "confirm" | "tap" | "assert" => {
            if text().is_none() {
                let expected = match kind.as_str() {
                    "ai" => "expected a sub-goal text",
                    "confirm" => "expected a question text",
                    "tap" => "expected a target description",
                    _ => "expected the text the screen must show",
                };
                bad_step(
                    doc,
                    number,
                    kind,
                    key_line,
                    format!("{expected}{}", brace_hint(&[node])),
                );
            }
        }
        "wait" => {
            let raw = if node.is_scalar() {
                node.raw.as_deref().unwrap_or_default().trim()
            } else {
                ""
            };
            let millis = raw.parse::<i64>().ok();
            let valid = LONG_RE.is_match(raw)
                && millis.is_some_and(|millis| (0..=FlowParser::MAX_WAIT_MILLIS).contains(&millis));
            if !valid {
                bad_step(
                    doc,
                    number,
                    kind,
                    key_line,
                    format!(
                        "expected milliseconds as an integer 0..{}",
                        FlowParser::MAX_WAIT_MILLIS
                    ),
                );
            }
        }
        "type" => {
            if !type_ok(node) {
                let hint = match node.entries() {
                    Some(fields) => {
                        brace_hint(&fields.iter().map(|(_, value)| value).collect::<Vec<_>>())
                    }
                    None => "",
                };
                bad_step(
                    doc,
                    number,
                    kind,
                    key_line,
                    format!("expected {{ target: ..., text: ... }}{hint}"),
                );
            }
        }
        _ => bad_step(
            doc,
            number,
            kind,
            key_line,
            format!(
                "unknown step kind; expected one of {}",
                FlowParser::STEP_KINDS.join(", ")
            ),
        ),
    }
}

fn bad_step(doc: &mut Doc, number: usize, kind: &str, key_line: usize, message: String) {
    doc.problem(format!("step {number} ({kind}): {message}"), Some(key_line));
}

fn type_ok(node: &Node) -> bool {
    let Some(fields) = node.entries() else {
        return false;
    };
    if fields
        .iter()
        .any(|(key, _)| key != "target" && key != "text")
    {
        return false;
    }
    let target = fields
        .iter()
        .find(|(key, _)| key == "target")
        .map(|(_, node)| node);
    let text = fields
        .iter()
        .find(|(key, _)| key == "text")
        .map(|(_, node)| node);
    match (target, text) {
        (Some(target), Some(text)) => {
            target.is_scalar()
                && target
                    .raw
                    .as_deref()
                    .is_some_and(|raw| !raw.trim().is_empty())
                && text.is_scalar()
                && text.raw.is_some()
        }
        _ => false,
    }
}
