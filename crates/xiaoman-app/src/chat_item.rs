//! The chat list model and its presentation rules, mirroring
//! The transcript rows plus the screen-free parts of the chat timeline and components:
//! [`chat_timeline`] folds adjacent progress entries, [`chat_rich_text`] applies the bold and
//! list markers, and [`approval_line`] is the row the approval card leaves behind.
//!
//! Everything here returns text and ranges; nothing needs a toolkit.

use xiaoman_api::{Action, AgentEvent, ApprovalAnswer, ApprovalScope};

/// One row of the chat list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatItem {
    User {
        id: i64,
        text: String,
    },
    Assistant {
        id: i64,
        text: String,
    },
    /// Compact progress line: steps, skill chosen, memory updated, task result.
    Progress {
        id: i64,
        text: String,
        ok: Option<bool>,
    },
    Question {
        id: i64,
        text: String,
        takeover: bool,
    },
    Error {
        id: i64,
        text: String,
    },
}

impl ChatItem {
    pub fn id(&self) -> i64 {
        match self {
            Self::User { id, .. }
            | Self::Assistant { id, .. }
            | Self::Progress { id, .. }
            | Self::Question { id, .. }
            | Self::Error { id, .. } => *id,
        }
    }
}

/// Human-readable (Chinese) text for agent events, used by the chat list and the status overlay.
pub mod step_text {
    use super::*;

    /// `StepText.action`.
    pub fn action(action: &Action) -> String {
        match action {
            Action::Tap {
                target, node_id, ..
            } => format!("点击{}", at(target.as_deref(), *node_id)),
            Action::LongPress {
                target, node_id, ..
            } => format!("长按{}", at(target.as_deref(), *node_id)),
            Action::Swipe { target, .. } => format!("滑动{}", at(target.as_deref(), None)),
            Action::TypeText {
                text,
                target,
                node_id,
                ..
            } => {
                format!("输入「{text}」{}", at(target.as_deref(), *node_id))
            }
            Action::Back { .. } => "返回".to_string(),
            Action::Home { .. } => "回到桌面".to_string(),
            Action::LaunchApp { package_name, .. } => format!("打开 {package_name}"),
            Action::OpenDeeplink { uri, .. } => {
                format!(
                    "打开链接 {}",
                    uri.split_once('?')
                        .map_or(uri.as_str(), |(before, _)| before)
                )
            }
            Action::Wait { millis, .. } => format!("等待 {millis} 毫秒"),
            Action::AskUser { question, .. } => format!("询问：{question}"),
            Action::Finish { summary, .. } => format!("结束：{summary}"),
        }
    }

    fn at(target: Option<&str>, node_id: Option<i32>) -> String {
        match target {
            Some(target) if !target.trim().is_empty() => format!(" {target}"),
            _ => match node_id {
                Some(id) => format!(" #{id}"),
                None => String::new(),
            },
        }
    }

    /// Overlay / notification status for `event`, or `None` to keep the previous one.
    pub fn status(event: &AgentEvent) -> Option<String> {
        match event {
            AgentEvent::PhoneTaskStarted { goal, .. } => Some(format!("开始：{goal}")),
            AgentEvent::StepPlanned {
                step_index,
                action: planned,
                ..
            } => Some(format!("第 {} 步：{}", step_index + 1, action(planned))),
            AgentEvent::StepBlocked { step_index, .. } => {
                Some(format!("第 {} 步已拦截", step_index + 1))
            }
            AgentEvent::NeedsUser {
                question, takeover, ..
            } => Some(if *takeover {
                "请接管手机，完成后回到小满回复".to_string()
            } else {
                format!("等待你回答：{question}")
            }),
            AgentEvent::PhoneTaskFinished { success, .. } => {
                Some(if *success { "已完成" } else { "未完成" }.to_string())
            }
            _ => None,
        }
    }

    /// The chat row for `event`, or `None` when it only updates the status.
    pub fn chat_item(event: &AgentEvent, id: i64) -> Option<ChatItem> {
        let item = match event {
            AgentEvent::Reply { text, .. } => ChatItem::Assistant {
                id,
                text: text.clone(),
            },
            AgentEvent::SkillChosen { skill_name, .. } => ChatItem::Progress {
                id,
                text: format!("使用技能 {skill_name}"),
                ok: None,
            },
            AgentEvent::PhoneTaskStarted { goal, .. } => ChatItem::Progress {
                id,
                text: format!("开始手机任务：{goal}"),
                ok: None,
            },
            AgentEvent::StepPlanned { .. } => return None,
            AgentEvent::StepExecuted {
                step_index,
                action: executed,
                result,
                ..
            } => {
                let failure = match (&result.message, result.ok) {
                    (Some(message), false) => format!("（{message}）"),
                    _ => String::new(),
                };
                ChatItem::Progress {
                    id,
                    text: format!("{}. {}{failure}", step_index + 1, action(executed)),
                    ok: Some(result.ok),
                }
            }
            AgentEvent::StepBlocked {
                step_index,
                action: blocked,
                decision,
                ..
            } => ChatItem::Progress {
                id,
                text: format!(
                    "{}. {}：已拦截（{}）",
                    step_index + 1,
                    action(blocked),
                    decision.reason()
                ),
                ok: Some(false),
            },
            AgentEvent::NeedsUser {
                question, takeover, ..
            } => ChatItem::Question {
                id,
                text: question.clone(),
                takeover: *takeover,
            },
            AgentEvent::MemoryUpdated { permalink, .. } => ChatItem::Progress {
                id,
                text: format!("已更新记忆 {permalink}"),
                ok: None,
            },
            AgentEvent::ToolCalled { summary, ok, .. } => ChatItem::Progress {
                id,
                text: summary.clone(),
                ok: Some(*ok),
            },
            AgentEvent::PhoneTaskFinished {
                success,
                summary,
                steps,
                ..
            } => ChatItem::Progress {
                id,
                text: format!(
                    "{}（{steps} 步）：{summary}",
                    if *success {
                        "任务完成"
                    } else {
                        "任务未完成"
                    }
                ),
                ok: Some(*success),
            },
            AgentEvent::Failed { message, .. } => ChatItem::Error {
                id,
                text: message.clone(),
            },
        };
        Some(item)
    }
}

/// Texts the session adds to the chat itself; the Kotlin `SessionTexts` defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalTexts {
    /// `{prompt}` is replaced with the approval prompt.
    pub approved_once: String,
    pub approved_task: String,
    pub denied: String,
}

impl Default for ApprovalTexts {
    fn default() -> Self {
        Self {
            approved_once: "已允许：{prompt}".to_string(),
            approved_task: "本次任务都允许：{prompt}".to_string(),
            denied: "已拒绝：{prompt}".to_string(),
        }
    }
}

/// The progress row an answered approval card adds.
///
/// The card itself shows while the session holds a pending approval; once answered, the
/// transcript keeps exactly this one line with the verdict and never reopens the card.
pub fn approval_line(
    id: i64,
    prompt: &str,
    answer: ApprovalAnswer,
    texts: &ApprovalTexts,
) -> ChatItem {
    let template = if !answer.approved {
        &texts.denied
    } else if answer.scope == ApprovalScope::Task {
        &texts.approved_task
    } else {
        &texts.approved_once
    };
    let text = template.replace("{prompt}", prompt);
    ChatItem::Progress {
        id,
        text,
        ok: Some(answer.approved),
    }
}

// ---------------------------------------------------------------------------------------------
// Timeline folding
// ---------------------------------------------------------------------------------------------

/// A row of the transcript: one message, or the run of adjacent progress entries before it.
///
/// `Process` only ever holds [`ChatItem::Progress`] items; [`chat_timeline`] is the only
/// constructor and the fold keeps that invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineRow {
    Message(ChatItem),
    Process(Vec<ChatItem>),
}

impl TimelineRow {
    pub fn id(&self) -> i64 {
        match self {
            Self::Message(item) => item.id(),
            // The first entry's id is the card's identity, so appending progress keeps it stable.
            Self::Process(entries) => entries.first().map(ChatItem::id).unwrap_or_default(),
        }
    }

    /// Every item the row shows, in chat order; folding then unfolding is lossless.
    pub fn items(&self) -> Vec<ChatItem> {
        match self {
            Self::Message(item) => vec![item.clone()],
            Self::Process(entries) => entries.clone(),
        }
    }
}

/// Fold only adjacent diagnostic entries. Questions, failures and replies always remain visible.
pub fn chat_timeline(items: &[ChatItem]) -> Vec<TimelineRow> {
    let mut rows: Vec<TimelineRow> = Vec::new();
    let mut pending: Vec<ChatItem> = Vec::new();
    for item in items {
        match item {
            ChatItem::Progress { .. } => pending.push(item.clone()),
            _ => {
                flush(&mut rows, &mut pending);
                rows.push(TimelineRow::Message(item.clone()));
            }
        }
    }
    flush(&mut rows, &mut pending);
    rows
}

fn flush(rows: &mut Vec<TimelineRow>, pending: &mut Vec<ChatItem>) {
    if !pending.is_empty() {
        rows.push(TimelineRow::Process(std::mem::take(pending)));
    }
}

// ---------------------------------------------------------------------------------------------
// Rich text
// ---------------------------------------------------------------------------------------------

/// A rendered chat message: the plain `text`, plus the byte ranges drawn in bold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichText {
    pub text: String,
    pub spans: Vec<TextSpan>,
}

/// A bold run of [`RichText::text`], as byte offsets usable for `&text[start..end]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
}

/// `chatRichText`: heading markers are dropped, `- ` and `* ` become `• `, `**bold**` becomes a
/// span. Unmatched `**` stays literal.
///
/// One deliberate difference from the Kotlin regexes: a marker followed only by whitespace at the
/// end of a line keeps the line break instead of swallowing the next line, because the text is
/// scanned line by line.
pub fn chat_rich_text(raw: &str) -> RichText {
    let stripped = strip_markers(raw);
    let bytes = stripped.as_bytes();
    let mut text = String::with_capacity(stripped.len());
    let mut spans: Vec<TextSpan> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"**") {
            let after = index + 2;
            if let Some(close) = stripped[after..].find("**") {
                let end = after + close;
                let inner = &stripped[after..end];
                if !inner.is_empty() && !inner.contains('\n') {
                    let start = text.len();
                    text.push_str(inner);
                    spans.push(TextSpan {
                        start,
                        end: text.len(),
                    });
                    index = end + 2;
                    continue;
                }
            }
            // Not a bold run: keep one asterisk and retry from the next byte, exactly where the
            // Kotlin regex engine would retry.
        }
        let character = stripped[index..]
            .chars()
            .next()
            .expect("index stays on a char boundary");
        text.push(character);
        index += character.len_utf8();
    }
    RichText { text, spans }
}

/// The two `replace` calls at the top of `chatRichText`.
fn strip_markers(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for (line_index, line) in raw.split('\n').enumerate() {
        if line_index > 0 {
            out.push('\n');
        }
        out.push_str(&strip_line_marker(line));
    }
    out
}

fn strip_line_marker(line: &str) -> String {
    let hashes = line.chars().take_while(|value| *value == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &line[hashes..];
        let trimmed = rest.trim_start();
        if trimmed.len() != rest.len() {
            return trimmed.to_string();
        }
    }
    if let Some(first) = line.chars().next() {
        if first == '-' || first == '*' {
            let rest = &line[1..];
            if rest.starts_with(char::is_whitespace) {
                return format!("• {}", rest.trim_start());
            }
        }
    }
    line.to_string()
}
