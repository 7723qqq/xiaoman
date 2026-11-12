//! Parses `flow.yaml`. Pure: text in, flow and problems out. The flow is `None` whenever any
//! problem was found, so a half-understood flow never runs.

use xiaoman_api::{Flow, FlowStep, SkillProblem};

use crate::yaml::{DEEPLINK_FORM, Node, PACKAGE_NAME, YamlDoc};

pub struct FlowParser;

impl FlowParser {
    /// Longest accepted `wait`, in milliseconds.
    pub const MAX_WAIT_MILLIS: i64 = 600_000;

    pub const STEP_KINDS: [&'static str; 8] = [
        "launch", "deeplink", "ai", "confirm", "tap", "type", "wait", "assert",
    ];

    /// `deeplinks` are the skill's named deep links; a `deeplink:` step must name one or be a literal
    /// URI. When `apps` is non-empty, a `launch:` step must name one of them.
    pub fn parse(
        text: &str,
        source: &str,
        deeplinks: &[(String, String)],
        apps: &[String],
    ) -> (Option<Flow>, Vec<SkillProblem>) {
        let mut doc = YamlDoc::new(source, 0);
        let Some(root) = doc.compose(text) else {
            if doc.problems.is_empty() {
                doc.problem_at("flow.yaml is empty; expected 'steps:'", None);
            }
            return (None, doc.problems);
        };
        if root.mapping().is_none() {
            doc.problem("flow.yaml must be a mapping with 'steps:'", &root);
            return (None, doc.problems);
        }
        let entries = doc.entries(&root);
        for entry in &entries {
            if entry.key != "steps" {
                doc.problem_at(
                    format!("unknown key '{}'; only 'steps' is allowed", entry.key),
                    Some(entry.key_line),
                );
            }
        }
        let steps_entry = entries.iter().find(|entry| entry.key == "steps");
        match steps_entry {
            None => doc.problem("missing 'steps:' list", &root),
            Some(entry) => match entry.value.sequence() {
                None => doc.problem_at("'steps' must be a list", Some(entry.key_line)),
                Some([]) => doc.problem_at("'steps' is empty", Some(entry.key_line)),
                Some(_) => {}
            },
        }
        let mut steps = Vec::new();
        if let Some(items) = steps_entry.and_then(|entry| entry.value.sequence()) {
            for (index, node) in items.iter().enumerate() {
                if let Some(step) = parse_step(&mut doc, index + 1, node, deeplinks, apps) {
                    steps.push(step);
                }
            }
        }
        if doc.problems.is_empty() {
            (Some(Flow { steps }), doc.problems)
        } else {
            (None, doc.problems)
        }
    }
}

fn parse_step(
    doc: &mut YamlDoc,
    number: usize,
    node: &Node,
    deeplinks: &[(String, String)],
    apps: &[String],
) -> Option<FlowStep> {
    let one_pair = node.mapping().is_some_and(|entries| entries.len() == 1);
    if !one_pair {
        doc.problem(
            format!("step {number}: expected one 'kind: value' pair, e.g. '- tap: 确认呼叫'"),
            node,
        );
        return None;
    }
    let entry = doc.entries(node).into_iter().next()?;
    let kind = entry.key.clone();
    let key_line = entry.key_line;
    let value = entry.value;

    fn bad(
        doc: &mut YamlDoc,
        number: usize,
        kind: &str,
        key_line: usize,
        message: &str,
    ) -> Option<FlowStep> {
        doc.problem_at(format!("step {number} ({kind}): {message}"), Some(key_line));
        None
    }

    match kind.as_str() {
        "launch" => {
            let Some(package) = scalar_text(&value) else {
                return bad(doc, number, &kind, key_line, "expected a package name");
            };
            if !PACKAGE_NAME.is_match(&package) {
                return bad(
                    doc,
                    number,
                    &kind,
                    key_line,
                    &format!("'{package}' is not a package name"),
                );
            }
            if !apps.is_empty() && !apps.iter().any(|app| app == &package) {
                return bad(
                    doc,
                    number,
                    &kind,
                    key_line,
                    &format!("'{package}' is not in the skill's apps"),
                );
            }
            Some(FlowStep::Launch {
                package_name: package,
            })
        }
        "deeplink" => {
            let Some(target) = scalar_text(&value) else {
                return bad(
                    doc,
                    number,
                    &kind,
                    key_line,
                    "expected a deeplinks key or a URI",
                );
            };
            if deeplinks.iter().any(|(key, _)| key == &target) || DEEPLINK_FORM.is_match(&target) {
                Some(FlowStep::Deeplink { key_or_uri: target })
            } else {
                bad(
                    doc,
                    number,
                    &kind,
                    key_line,
                    &format!(
                        "'{target}' is neither a key of the skill's deeplinks nor a scheme:// URI"
                    ),
                )
            }
        }
        "ai" => match scalar_text(&value) {
            Some(goal) => Some(FlowStep::Ai { goal }),
            None => bad(doc, number, &kind, key_line, "expected a sub-goal text"),
        },
        "confirm" => match scalar_text(&value) {
            Some(question) => Some(FlowStep::Confirm { question }),
            None => bad(doc, number, &kind, key_line, "expected a question text"),
        },
        "tap" => match scalar_text(&value) {
            Some(target) => Some(FlowStep::Tap { target }),
            None => bad(
                doc,
                number,
                &kind,
                key_line,
                "expected a target description",
            ),
        },
        "assert" => match scalar_text(&value) {
            Some(expectation) => Some(FlowStep::Assert { expectation }),
            None => bad(
                doc,
                number,
                &kind,
                key_line,
                "expected the text the screen must show",
            ),
        },
        "wait" => {
            let millis = if value.is_null() {
                None
            } else {
                value
                    .scalar()
                    .map(str::trim)
                    .and_then(|text| text.parse::<i64>().ok())
            };
            match millis {
                Some(millis) if (0..=FlowParser::MAX_WAIT_MILLIS).contains(&millis) => {
                    Some(FlowStep::Wait { millis })
                }
                _ => bad(
                    doc,
                    number,
                    &kind,
                    key_line,
                    &format!(
                        "expected milliseconds as an integer 0..{}",
                        FlowParser::MAX_WAIT_MILLIS
                    ),
                ),
            }
        }
        "type" => match parse_type(&value) {
            Some(step) => Some(step),
            None => bad(
                doc,
                number,
                &kind,
                key_line,
                "expected { target: ..., text: ... }",
            ),
        },
        _ => bad(
            doc,
            number,
            &kind,
            key_line,
            &format!(
                "unknown step kind; expected one of {}",
                FlowParser::STEP_KINDS.join(", ")
            ),
        ),
    }
}

fn parse_type(value: &Node) -> Option<FlowStep> {
    let entries = value.mapping()?;
    let mut target = None;
    let mut text = None;
    for (key, item) in entries {
        match key.scalar()? {
            "target" => {
                target = match item {
                    Node::Scalar { value, .. } if !item.is_null() => Some(value.trim().to_string()),
                    _ => None,
                }
            }
            "text" => {
                text = match item {
                    Node::Scalar { .. } if !item.is_null() => item.scalar().map(str::to_string),
                    _ => None,
                }
            }
            _ => return None,
        }
    }
    match (target, text) {
        (Some(target), Some(text)) if !target.is_empty() => Some(FlowStep::Type { target, text }),
        _ => None,
    }
}

fn scalar_text(value: &Node) -> Option<String> {
    if value.is_null() {
        return None;
    }
    value
        .scalar()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}
