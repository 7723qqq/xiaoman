//! Parses and validates skill folders.

use std::sync::LazyLock;

use regex::Regex;
use xiaoman_api::{Flow, Skill, SkillChannel, SkillProblem};

use crate::flow_parser::FlowParser;
use crate::yaml::{DEEPLINK_FORM, Node, PACKAGE_NAME, YamlDoc};

pub struct SkillParser;

impl SkillParser {
    pub fn name_pattern() -> &'static Regex {
        static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9-]+$").unwrap());
        &NAME
    }

    pub const KEYS: [&'static str; 7] = [
        "name",
        "description",
        "apps",
        "channel",
        "confirm_before",
        "memory",
        "deeplinks",
    ];

    /// Parses one skill folder. The frontmatter `name` should equal the folder name.
    pub fn parse_folder(
        name: &str,
        skill_md: &str,
        flow_yaml: Option<&str>,
        source: &str,
    ) -> (Option<Skill>, Vec<SkillProblem>) {
        let (skill, mut problems) = Self::parse(skill_md, source);
        if let Some(skill) = &skill {
            if skill.name != name {
                problems.push(SkillProblem {
                    source: format!("{source}/SKILL.md"),
                    message: format!(
                        "name '{}' does not match folder name '{}'",
                        skill.name, name
                    ),
                    line: name_line(skill_md),
                });
            }
        }
        let Some(mut skill) = skill else {
            return (None, problems);
        };
        // A flow is checked against the skill's deeplinks and apps, so it is only parsed once SKILL.md is valid.
        let Some(flow_yaml) = flow_yaml else {
            return (Some(skill), problems);
        };
        if skill.channel == SkillChannel::Chat {
            problems.push(SkillProblem {
                source: format!("{source}/flow.yaml"),
                message: "聊天工作流不操作手机，不能设置 flow.yaml".to_string(),
                line: Some(1),
            });
            return (None, problems);
        }
        let (flow, flow_problems) = FlowParser::parse(
            flow_yaml,
            &format!("{source}/flow.yaml"),
            &skill.deeplinks,
            &skill.apps,
        );
        problems.extend(flow_problems);
        match flow {
            Some(flow) => {
                skill.flow = Some(flow);
                (Some(skill), problems)
            }
            None => (None, problems),
        }
    }

    /// Parses `SKILL.md` alone (no folder-name check, no flow).
    pub fn parse(skill_md: &str, source: &str) -> (Option<Skill>, Vec<SkillProblem>) {
        let mut doc = YamlDoc::new(&format!("{source}/SKILL.md"), 1);
        let Some((yaml, body)) = split_frontmatter(skill_md) else {
            doc.problem_at(
                "missing frontmatter: SKILL.md must start with a '---' line and close it with another '---'",
                Some(1),
            );
            return (None, doc.problems);
        };
        let root = doc.compose(&yaml);
        if let Some(root) = &root {
            if root.mapping().is_none() {
                doc.problem("frontmatter must be a mapping of keys", root);
                return (None, doc.problems);
            }
        }
        if !doc.problems.is_empty() {
            return (None, doc.problems);
        }
        let entries = root
            .as_ref()
            .map(|root| doc.entries(root))
            .unwrap_or_default();
        for entry in &entries {
            if !Self::KEYS.contains(&entry.key.as_str()) {
                doc.problem_at(
                    format!(
                        "unknown key '{}'; expected one of {}",
                        entry.key,
                        Self::KEYS.join(", ")
                    ),
                    Some(entry.key_line),
                );
            }
        }
        let find = |key: &str| entries.iter().find(|entry| entry.key == key);

        let name_entry = find("name");
        let name = name_entry
            .and_then(|entry| doc.string(entry))
            .map(|name| name.trim().to_string());
        match (&name_entry, &name) {
            (None, _) => doc.problem_at("missing required key 'name'", Some(1)),
            (Some(entry), Some(name)) if !Self::name_pattern().is_match(name) => {
                doc.problem_at(
                    format!("name '{name}' must match [a-z0-9-]+"),
                    Some(entry.key_line),
                );
            }
            _ => {}
        }

        let description_entry = find("description");
        let description = description_entry
            .and_then(|entry| doc.string(entry))
            .map(|description| description.trim().to_string());
        match (&description_entry, &description) {
            (None, _) => doc.problem_at("missing required key 'description'", Some(1)),
            (Some(entry), Some(description)) if description.is_empty() => {
                doc.problem_at("'description' must not be empty", Some(entry.key_line))
            }
            (Some(entry), Some(description)) if description.contains('\n') => {
                doc.problem_at("'description' must be a single line", Some(entry.key_line))
            }
            _ => {}
        }

        let mut apps = Vec::new();
        if let Some(entry) = find("apps") {
            if let Some(list) = doc.string_list(entry) {
                for app in &list {
                    if !PACKAGE_NAME.is_match(app) {
                        doc.problem_at(
                            format!("apps: '{app}' is not a package name"),
                            Some(entry.key_line),
                        );
                    }
                }
                apps = list;
            }
        }

        let mut channel = SkillChannel::Gui;
        if let Some(entry) = find("channel") {
            if let Some(wire) = doc.string(entry) {
                match SkillChannel::from_wire(&wire) {
                    Some(parsed) => channel = parsed,
                    None => {
                        let wires = [
                            "api",
                            "cli",
                            "deeplink",
                            "gui",
                            "deeplink-then-gui",
                            "mixed",
                            "chat",
                        ];
                        doc.problem_at(
                            format!(
                                "unknown channel '{wire}'; expected one of {}",
                                wires.join(", ")
                            ),
                            Some(entry.key_line),
                        );
                    }
                }
            }
        }

        let confirm_before = find("confirm_before")
            .and_then(|entry| doc.string_list(entry))
            .unwrap_or_default();
        let memory = find("memory")
            .and_then(|entry| doc.string_list(entry))
            .unwrap_or_default();
        let deeplinks = find("deeplinks")
            .map(|entry| parse_deeplinks(&mut doc, entry))
            .unwrap_or_default();

        if channel == SkillChannel::Chat {
            for key in ["apps", "deeplinks", "confirm_before"] {
                if let Some(entry) = find(key) {
                    doc.problem_at(
                        format!("聊天工作流不操作手机，不能设置 {key}"),
                        Some(entry.key_line),
                    );
                }
            }
        }

        let (Some(name), Some(description)) = (name, description) else {
            return (None, doc.problems);
        };
        if !doc.problems.is_empty() {
            return (None, doc.problems);
        }
        (
            Some(Skill {
                name,
                description,
                apps,
                channel,
                confirm_before,
                memory_refs: memory,
                deeplinks,
                instructions: body,
                flow: None::<Flow>,
                source: source.to_string(),
            }),
            Vec::new(),
        )
    }
}

fn parse_deeplinks(doc: &mut YamlDoc, entry: &crate::yaml::Entry) -> Vec<(String, String)> {
    if entry.value.is_null() {
        return Vec::new();
    }
    if entry.value.mapping().is_none() {
        doc.problem_at(
            "'deeplinks' must be a mapping of name: \"scheme://...\"",
            Some(entry.key_line),
        );
        return Vec::new();
    }
    let mut out = Vec::new();
    for link in doc.entries(&entry.value) {
        let value = match &link.value {
            Node::Scalar { value, .. } if !link.value.is_null() => Some(value.trim().to_string()),
            _ => None,
        };
        match value {
            _ if link.key.trim().is_empty() => {
                doc.problem_at("deeplinks: names must not be empty", Some(link.key_line))
            }
            None => doc.problem_at(
                format!("deeplinks: '{}' must be a string", link.key),
                Some(link.key_line),
            ),
            Some(value) if !DEEPLINK_FORM.is_match(&value) => doc.problem_at(
                format!(
                    "deeplinks: '{}' = '{}' is not a scheme:// URI",
                    link.key, value
                ),
                Some(link.key_line),
            ),
            Some(value) => out.push((link.key, value)),
        }
    }
    out
}

/// Splits `---\n<yaml>\n---\n<body>`; `None` when there is no closed frontmatter block.
pub fn split_frontmatter(text: &str) -> Option<(String, String)> {
    let normalized = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = normalized.lines().collect();
    if lines.first().map(|line| line.trim_end()) != Some("---") {
        return None;
    }
    let end = (1..lines.len()).find(|&index| matches!(lines[index].trim_end(), "---" | "..."))?;
    let yaml = lines[1..end].join("\n");
    let body = lines[end + 1..].join("\n").trim().to_string();
    Some((yaml, body))
}

fn name_line(skill_md: &str) -> Option<usize> {
    let normalized = skill_md.strip_prefix('\u{feff}').unwrap_or(skill_md);
    normalized
        .lines()
        .position(|line| line.starts_with("name:"))
        .map(|index| index + 1)
}
