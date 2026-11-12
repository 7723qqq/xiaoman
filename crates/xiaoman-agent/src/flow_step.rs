//! Runs a skill's `flow.yaml` steps; device actions go through `OperatorLoop::execute` and so
//! through Sentinel.

use std::sync::Arc;

use xiaoman_api::{Action, AgentEvent, ChatMessage, ChatRequest, Flow, FlowStep, UserChannel};

use crate::json_text::JsonText;
use crate::operator_loop::OperatorLoop;
use crate::prompts;
use crate::screen_text;
use crate::task_run::{Executed, LoopOutcome, StepConstraint, TaskRun};

const SINGLE_STEP_TRIES: i32 = 3;

pub async fn run(
    loop_: &OperatorLoop,
    users: &Arc<dyn UserChannel>,
    task: &mut TaskRun,
    flow: &Flow,
) -> LoopOutcome {
    let mut summary: Option<String> = None;
    for step in &flow.steps {
        if task.stop.stopped() {
            return LoopOutcome {
                success: false,
                summary: prompts::STOPPED.to_string(),
            };
        }
        let outcome = match step {
            FlowStep::Launch { package_name } => {
                let action = Action::LaunchApp {
                    package_name: package_name.clone(),
                    target: Some(format!("打开 {package_name}")),
                    reason: None,
                };
                act(loop_, task, &action).await
            }
            FlowStep::Deeplink { key_or_uri } => {
                let template = task
                    .skill
                    .as_ref()
                    .and_then(|skill| skill.deeplinks.iter().find(|(key, _)| key == key_or_uri))
                    .map(|(_, value)| value.clone())
                    .unwrap_or_else(|| key_or_uri.clone());
                let uri = fill(loop_, users, task, &template, true).await;
                let label = (template != *key_or_uri).then(|| key_or_uri.clone());
                let action = Action::OpenDeeplink {
                    uri,
                    package_name: task
                        .skill
                        .as_ref()
                        .and_then(|skill| skill.apps.first().cloned()),
                    target: label,
                    reason: None,
                };
                act(loop_, task, &action).await
            }
            FlowStep::Ai { goal } => {
                let goal = fill(loop_, users, task, goal, false).await;
                loop_.run(task, Some(&goal), None, loop_.max_steps()).await
            }
            FlowStep::Confirm { question } => confirm(users, task, question).await,
            FlowStep::Tap { target } => {
                let constraint = StepConstraint {
                    kind: "tap".into(),
                    target: target.clone(),
                    text: None,
                };
                loop_
                    .run(task, None, Some(&constraint), SINGLE_STEP_TRIES)
                    .await
            }
            FlowStep::Type { target, text } => {
                let text = fill(loop_, users, task, text, false).await;
                let constraint = StepConstraint {
                    kind: "type".into(),
                    target: target.clone(),
                    text: Some(text),
                };
                loop_
                    .run(task, None, Some(&constraint), SINGLE_STEP_TRIES)
                    .await
            }
            FlowStep::Wait { millis } => {
                let action = Action::Wait {
                    millis: *millis,
                    target: None,
                    reason: None,
                };
                act(loop_, task, &action).await
            }
            FlowStep::Assert { expectation } => loop_.check(task, expectation).await,
        };
        if !outcome.success {
            return outcome;
        }
        if matches!(step, FlowStep::Ai { .. }) {
            summary = Some(outcome.summary);
        }
    }
    LoopOutcome {
        success: true,
        summary: summary.unwrap_or_else(|| prompts::FLOW_DONE.to_string()),
    }
}

async fn act(loop_: &OperatorLoop, task: &mut TaskRun, action: &Action) -> LoopOutcome {
    task.emit(AgentEvent::StepPlanned {
        task_id: task.task_id.clone(),
        step_index: task.steps,
        action: action.clone(),
    });
    match loop_.execute(task, action, None).await {
        Executed::Blocked { decision } => LoopOutcome {
            success: false,
            summary: format!("操作被拦截：{}", decision.reason()),
        },
        Executed::Done { result } => LoopOutcome {
            success: result.ok,
            summary: if result.ok {
                screen_text::describe(action)
            } else {
                result.message.unwrap_or_else(|| "执行失败".to_string())
            },
        },
    }
}

async fn confirm(users: &Arc<dyn UserChannel>, task: &mut TaskRun, question: &str) -> LoopOutcome {
    task.emit(AgentEvent::NeedsUser {
        task_id: task.task_id.clone(),
        question: question.to_string(),
        takeover: false,
    });
    let answer = users.ask(&task.task_id, question, false).await;
    if crate::flow_runner::is_yes(&answer) {
        LoopOutcome {
            success: true,
            summary: answer,
        }
    } else {
        LoopOutcome {
            success: false,
            summary: prompts::CONFIRM_DECLINED.to_string(),
        }
    }
}

/// Replaces `{name}` placeholders: values already known in the task, else asked from the operator
/// model as `{"values": {...}}`, else asked from the user.
pub async fn fill(
    loop_: &OperatorLoop,
    users: &Arc<dyn UserChannel>,
    task: &mut TaskRun,
    template: &str,
    url_encode: bool,
) -> String {
    let names = placeholders(template);
    if names.is_empty() {
        return template.to_string();
    }
    let missing: Vec<String> = names
        .into_iter()
        .filter(|name| !task.values.contains_key(name))
        .collect();
    if !missing.is_empty() {
        let request = ChatRequest {
            messages: vec![
                ChatMessage::system(prompts::VALUES_SYSTEM),
                ChatMessage::user(prompts::values_request(
                    &task.goal,
                    &missing,
                    &task.references,
                )),
            ],
            tools: Vec::new(),
            json_mode: true,
        };
        let values = match loop_.complete_with_retry(request).await {
            Ok(reply) => JsonText::first_object(&reply.text).map(|root| match root.get("values") {
                Some(serde_json::Value::Object(values)) => values.clone(),
                _ => root,
            }),
            Err(_) => None,
        };
        for name in missing {
            let value = match values
                .as_ref()
                .and_then(|values| JsonText::string(values, &name))
            {
                Some(value) => value,
                None => {
                    let question = prompts::ask_value(&name);
                    task.emit(AgentEvent::NeedsUser {
                        task_id: task.task_id.clone(),
                        question: question.clone(),
                        takeover: false,
                    });
                    users
                        .ask(&task.task_id, &question, false)
                        .await
                        .trim()
                        .to_string()
                }
            };
            task.values.insert(name, value);
        }
    }
    replace_placeholders(template, &task.values, url_encode)
}

fn placeholders(template: &str) -> Vec<String> {
    let chars: Vec<char> = template.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '{' {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < chars.len()
            && (chars[end].is_alphanumeric() || chars[end] == '_' || chars[end] == '-')
        {
            end += 1;
        }
        if end < chars.len() && chars[end] == '}' && end > index + 1 {
            let name: String = chars[index + 1..end].iter().collect();
            if !out.contains(&name) {
                out.push(name);
            }
            index = end + 1;
        } else {
            index += 1;
        }
    }
    out
}

fn replace_placeholders(
    template: &str,
    values: &std::collections::HashMap<String, String>,
    url_encode: bool,
) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '{' {
            let mut end = index + 1;
            while end < chars.len()
                && (chars[end].is_alphanumeric() || chars[end] == '_' || chars[end] == '-')
            {
                end += 1;
            }
            if end < chars.len() && chars[end] == '}' && end > index + 1 {
                let name: String = chars[index + 1..end].iter().collect();
                let value = values.get(&name).map(String::as_str).unwrap_or("");
                if url_encode {
                    out.push_str(&url_encode_value(value));
                } else {
                    out.push_str(value);
                }
                index = end + 1;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn url_encode_value(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '*') {
            out.push(c);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
