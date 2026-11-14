//! The chat screen's rows: the transcript, the run status, the approval card and the skill draft
//! card, with no toolkit.
//!
//! The folding rules themselves already exist in `xiaoman_app::chat_item` (`chat_timeline`,
//! `chat_rich_text`, `step_text`, `approval_line`) and are reused here rather than rewritten: this
//! module decides *when* a card shows, *what* the header says and *which* rows the screen paints.

use std::collections::BTreeSet;

use xiaoman_api::{AgentEvent, ApprovalAnswer, ApprovalScope, Decision, SkillAction};
use xiaoman_app::chat_item::{
    ApprovalTexts, ChatItem, RichText, TimelineRow, approval_line, chat_rich_text, chat_timeline,
    step_text,
};
use xiaoman_app::session::SessionSnapshot;

use super::copy;

/// One entry of the folded process card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEntry {
    pub text: String,
    /// `Some(false)` marks a failed step, which the Compose card paints in the error color.
    pub ok: Option<bool>,
}

impl ProgressEntry {
    pub fn failed(&self) -> bool {
        self.ok == Some(false)
    }
}

/// A question the agent is waiting on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingQuestion {
    pub id: i64,
    pub text: String,
    pub takeover: bool,
}

/// The approval card's state, pinned above the input row exactly as `ApprovalCard.kt` is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalCard {
    pub id: i64,
    pub title: String,
    pub prompt: String,
    pub reason: String,
    /// "回答中": both buttons are disabled while an answer is on its way back.
    pub responding: bool,
    /// The "更多选项" row is expanded, revealing the task-wide scope.
    pub more: bool,
}

impl ApprovalCard {
    /// The card waits only for `Decision::Ask`.
    ///
    /// `Allow` runs without asking and `Deny` never reaches the user, so neither opens a card;
    /// the `StepBlocked` progress row already says what happened.
    pub fn for_decision(id: i64, decision: &Decision) -> Option<Self> {
        match decision {
            Decision::Ask { prompt, reason } => Some(Self {
                id,
                title: copy::APPROVAL_TITLE.to_string(),
                prompt: prompt.clone(),
                reason: reason.clone(),
                responding: false,
                more: false,
            }),
            Decision::Allow { .. } | Decision::Deny { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    User,
    Assistant,
    Question { takeover: bool },
    Error,
}

/// One message row of the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub id: i64,
    pub kind: MessageKind,
    /// The small caption above the bubble: `小满`, `需要你帮一下`, `再告诉我一点`, `这次没完成`.
    /// `None` for the user's own bubble, which carries no caption.
    pub title: Option<String>,
    /// The body, with `**bold**` already converted to spans and list markers to `• `.
    pub body: RichText,
}

/// The folded run of adjacent progress entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRow {
    /// The first entry's id, so appending progress never changes the card's identity.
    pub id: i64,
    pub header: String,
    pub entries: Vec<ProgressEntry>,
    pub expanded: bool,
}

impl ProcessRow {
    pub fn failed(&self) -> bool {
        self.entries.iter().any(ProgressEntry::failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatRow {
    Message(MessageRow),
    Process(ProcessRow),
}

impl ChatRow {
    pub fn id(&self) -> i64 {
        match self {
            Self::Message(row) => row.id,
            Self::Process(row) => row.id,
        }
    }
}

/// The header chip: `等你确认` / `处理中` / `已就绪` / `待设置`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderState {
    pub title: String,
    pub subtitle: String,
    pub status: String,
    pub attention: bool,
    pub busy: bool,
    pub ready: bool,
}

/// A banner above the transcript, with its action label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner {
    pub text: String,
    pub action: String,
}

/// What `AgentSession.submit` reports back, so the screen knows whether to clear the draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submit {
    /// The text answered a pending question.
    Answered,
    /// A new turn started.
    Started,
    Busy,
    NotConfigured,
    Empty,
}

impl Submit {
    /// The input clears unless the session was busy or the text was blank, like `ChatInput.send`.
    pub fn clears_draft(self) -> bool {
        !matches!(self, Self::Busy | Self::Empty)
    }
}

/// The skill draft card: the full `SKILL.md` and what saving will do.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillDraftCard {
    pub id: i64,
    pub action: SkillAction,
    pub name: String,
    pub content: String,
    pub previous: Option<String>,
    pub shadows_bundled: bool,
}

/// The chat screen's state: the transcript, the run status and the two kinds of pending prompt.
#[derive(Debug, Clone)]
pub struct ChatModel {
    items: Vec<ChatItem>,
    next_id: i64,
    busy: bool,
    status: Option<String>,
    question: Option<PendingQuestion>,
    approval: Option<ApprovalCard>,
    skill_draft: Option<SkillDraftCard>,
    expanded: BTreeSet<i64>,
    configured: bool,
    /// The Kotlin header requires the accessibility service as well; this port has no Android
    /// service, so the flag is set once at startup and defaults to `true`.
    platform_ready: bool,
    approval_texts: ApprovalTexts,
}

impl Default for ChatModel {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
            busy: false,
            status: None,
            question: None,
            approval: None,
            skill_draft: None,
            expanded: BTreeSet::new(),
            configured: false,
            platform_ready: true,
            approval_texts: ApprovalTexts::default(),
        }
    }
}

impl ChatModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// A transcript already in hand, for tests and for seeding a screen.
    pub fn with_items(items: Vec<ChatItem>) -> Self {
        let next_id = items.iter().map(ChatItem::id).max().unwrap_or(0) + 1;
        Self {
            items,
            next_id,
            ..Self::default()
        }
    }

    pub fn configured(mut self, configured: bool) -> Self {
        self.configured = configured;
        self
    }

    pub fn set_configured(&mut self, configured: bool) {
        self.configured = configured;
    }

    pub fn is_configured(&self) -> bool {
        self.configured
    }

    pub fn set_platform_ready(&mut self, ready: bool) {
        self.platform_ready = ready;
    }

    pub fn items(&self) -> &[ChatItem] {
        &self.items
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// The line above the input row: the last step, or `正在想办法…` while a step has none.
    pub fn status_line(&self) -> Option<&str> {
        if self.busy {
            Some(self.status.as_deref().unwrap_or(copy::CHAT_THINKING))
        } else {
            None
        }
    }

    pub fn question(&self) -> Option<&PendingQuestion> {
        self.question.as_ref()
    }

    pub fn approval(&self) -> Option<&ApprovalCard> {
        self.approval.as_ref()
    }

    pub fn skill_draft(&self) -> Option<&SkillDraftCard> {
        self.skill_draft.as_ref()
    }

    fn take_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn push(&mut self, item: ChatItem) {
        self.items.push(item);
        // `AgentSession.MAX_ITEMS`: the transcript keeps the newest rows only.
        const MAX_ITEMS: usize = 500;
        if self.items.len() > MAX_ITEMS {
            let overflow = self.items.len() - MAX_ITEMS;
            self.items.drain(..overflow);
        }
    }

    /// Adds one item without going through the event mapping, for tests and seeded screens.
    pub fn push_item(&mut self, item: ChatItem) {
        if item.id() >= self.next_id {
            self.next_id = item.id() + 1;
        }
        self.push(item);
    }

    /// `AgentSession.listener`: status text first, then the transcript row, then the prompts.
    pub fn apply(&mut self, event: &AgentEvent) {
        if let Some(status) = step_text::status(event) {
            self.status = Some(status);
        }

        let id = self.take_id();
        if let Some(item) = step_text::chat_item(event, id) {
            if let ChatItem::Question { text, takeover, .. } = &item {
                self.question = Some(PendingQuestion {
                    id,
                    text: text.clone(),
                    takeover: *takeover,
                });
            }
            self.push(item);
        }

        match event {
            AgentEvent::PhoneTaskStarted { .. } => self.busy = true,
            AgentEvent::StepBlocked { decision, .. } => {
                if let Some(card) = ApprovalCard::for_decision(self.take_id(), decision) {
                    self.status = Some(format!("等待确认：{}", card.prompt));
                    self.approval = Some(card);
                }
            }
            AgentEvent::PhoneTaskFinished { .. } => self.finish(),
            _ => {}
        }
    }

    /// Mirrors a live [`xiaoman_app::session::Session`]: the session owns the transcript, the run
    /// flag and both pending prompts, so the screen only has to copy them over.
    ///
    /// The user's expanded process cards survive, and an approval card that is still the same one
    /// keeps its "更多选项" state.
    pub fn apply_snapshot(&mut self, snapshot: &SessionSnapshot) {
        self.items = snapshot.items.clone();
        self.next_id = self.items.iter().map(ChatItem::id).max().unwrap_or(0) + 1;
        self.busy = snapshot.busy;
        self.status = snapshot.status.clone();
        self.configured = snapshot.configured;
        self.question = snapshot.question.as_ref().map(|question| PendingQuestion {
            id: question.id,
            text: question.text.clone(),
            takeover: question.takeover,
        });
        let keep_more = self
            .approval
            .as_ref()
            .filter(|card| {
                snapshot
                    .approval
                    .as_ref()
                    .is_some_and(|approval| approval.id == card.id)
            })
            .map(|card| card.more)
            .unwrap_or(false);
        self.approval = snapshot.approval.as_ref().map(|approval| ApprovalCard {
            id: approval.id,
            title: copy::APPROVAL_TITLE.to_string(),
            prompt: approval.prompt.clone(),
            reason: approval.reason.clone(),
            responding: false,
            more: keep_more,
        });
        self.skill_draft = snapshot.skill_draft.as_ref().map(|draft| SkillDraftCard {
            id: draft.id,
            action: draft.action,
            name: draft.name.clone(),
            content: draft.content.clone(),
            previous: draft.previous.clone(),
            shadows_bundled: draft.shadows_bundled,
        });
    }

    /// Ends the run: `AgentSession.launch`'s `finally` block.
    pub fn finish(&mut self) {
        self.busy = false;
        self.status = None;
        self.question = None;
        self.approval = None;
        self.skill_draft = None;
    }

    /// The chat input: answers a pending question, else starts a turn.
    pub fn submit(&mut self, text: &str) -> Submit {
        let message = text.trim();
        if message.is_empty() {
            return Submit::Empty;
        }
        if self.question.take().is_some() {
            let id = self.take_id();
            self.push(ChatItem::User {
                id,
                text: message.to_string(),
            });
            return Submit::Answered;
        }
        if !self.configured {
            let id = self.take_id();
            self.push(ChatItem::User {
                id,
                text: message.to_string(),
            });
            let id = self.take_id();
            self.push(ChatItem::Error {
                id,
                text: copy::SESSION_NOT_CONFIGURED.to_string(),
            });
            return Submit::NotConfigured;
        }
        if self.busy {
            return Submit::Busy;
        }
        let id = self.take_id();
        self.push(ChatItem::User {
            id,
            text: message.to_string(),
        });
        self.busy = true;
        self.status = None;
        Submit::Started
    }

    /// `AgentSession.stop`: cooperative stop, pending prompts released.
    pub fn stop(&mut self) {
        if !self.busy {
            return;
        }
        self.status = Some("已停止".to_string());
        self.question = None;
        self.approval = None;
        self.skill_draft = None;
    }

    /// Opens the approval card for a decision, if the decision asks the user.
    pub fn request_approval(&mut self, decision: &Decision) -> bool {
        match ApprovalCard::for_decision(self.take_id(), decision) {
            Some(card) => {
                self.status = Some(format!("等待确认：{}", card.prompt));
                self.approval = Some(card);
                true
            }
            None => false,
        }
    }

    /// "更多选项" / the task-wide button.
    pub fn set_more(&mut self, more: bool) {
        if let Some(card) = self.approval.as_mut() {
            card.more = more;
        }
    }

    pub fn set_responding(&mut self, responding: bool) {
        if let Some(card) = self.approval.as_mut() {
            card.responding = responding;
        }
    }

    pub fn approve(&mut self, scope: ApprovalScope) {
        self.answer(ApprovalAnswer {
            approved: true,
            scope,
        });
    }

    pub fn deny(&mut self) {
        self.answer(ApprovalAnswer {
            approved: false,
            scope: ApprovalScope::Once,
        });
    }

    /// `AgentSession.request`: the answer becomes one progress line, and the card closes.
    pub fn answer(&mut self, answer: ApprovalAnswer) {
        let Some(card) = self.approval.take() else {
            return;
        };
        let id = self.take_id();
        let line = approval_line(id, &card.prompt, answer, &self.approval_texts);
        self.push(line);
        self.status = None;
    }

    pub fn is_expanded(&self, row_id: i64) -> bool {
        self.expanded.contains(&row_id)
    }

    pub fn toggle(&mut self, row_id: i64) {
        if !self.expanded.remove(&row_id) {
            self.expanded.insert(row_id);
        }
    }

    pub fn set_expanded(&mut self, row_id: i64, expanded: bool) {
        if expanded {
            self.expanded.insert(row_id);
        } else {
            self.expanded.remove(&row_id);
        }
    }

    pub fn rows(&self) -> Vec<ChatRow> {
        rows_for(&self.items, &self.expanded)
    }

    pub fn header(&self) -> HeaderState {
        let attention = self.approval.is_some() || self.question.is_some();
        let ready = self.configured && self.platform_ready;
        let status = if attention {
            copy::STATUS_ATTENTION
        } else if self.busy {
            copy::STATUS_BUSY
        } else if ready {
            copy::STATUS_READY
        } else {
            copy::STATUS_UNCONFIGURED
        };
        HeaderState {
            title: copy::APP_NAME.to_string(),
            subtitle: copy::APP_TAGLINE.to_string(),
            status: status.to_string(),
            attention,
            busy: self.busy,
            ready,
        }
    }

    /// The banners Compose shows above the transcript. The accessibility and notification
    /// banners are Android permission prompts and have no desktop equivalent.
    pub fn banners(&self) -> Vec<Banner> {
        let mut banners = Vec::new();
        if !self.configured {
            banners.push(Banner {
                text: copy::BANNER_MODELS.to_string(),
                action: copy::BANNER_MODELS_ACTION.to_string(),
            });
        }
        banners
    }

    /// `ChatInput.canSend`: a non-blank draft, and either an idle session or a question to answer.
    pub fn can_send(&self, draft: &str) -> bool {
        !draft.trim().is_empty() && (!self.busy || self.question.is_some())
    }

    pub fn placeholder(&self) -> &'static str {
        if self.question.is_some() {
            copy::CHAT_ANSWER_PLACEHOLDER
        } else {
            copy::CHAT_PLACEHOLDER
        }
    }

    /// The button is Stop while a run is busy and nothing is being asked.
    pub fn shows_stop(&self) -> bool {
        self.busy && self.question.is_none()
    }
}

/// `ChatTimeline.chatTimeline`: the rows for `items`, with the given process cards open.
pub fn rows_for(items: &[ChatItem], expanded: &BTreeSet<i64>) -> Vec<ChatRow> {
    chat_timeline(items)
        .into_iter()
        .filter_map(|row| match row {
            TimelineRow::Message(item) => message_row(&item).map(ChatRow::Message),
            TimelineRow::Process(entries) => {
                Some(ChatRow::Process(process_row(&entries, expanded)))
            }
        })
        .collect()
}

/// The message row for one item; `None` for progress, which belongs to a process card.
pub fn message_row(item: &ChatItem) -> Option<MessageRow> {
    let (kind, title, text) = match item {
        ChatItem::User { text, .. } => (MessageKind::User, None, text.as_str()),
        ChatItem::Assistant { text, .. } => (
            MessageKind::Assistant,
            Some(copy::CHAT_ASSISTANT_NAME.to_string()),
            text.as_str(),
        ),
        ChatItem::Question { text, takeover, .. } => (
            MessageKind::Question {
                takeover: *takeover,
            },
            Some(
                if *takeover {
                    copy::CHAT_QUESTION_TAKEOVER_TITLE
                } else {
                    copy::CHAT_QUESTION_TITLE
                }
                .to_string(),
            ),
            text.as_str(),
        ),
        ChatItem::Error { text, .. } => (
            MessageKind::Error,
            Some(copy::CHAT_ERROR_TITLE.to_string()),
            text.as_str(),
        ),
        ChatItem::Progress { .. } => return None,
    };
    Some(MessageRow {
        id: item.id(),
        kind,
        title,
        body: chat_rich_text(text),
    })
}

fn process_row(entries: &[ChatItem], expanded: &BTreeSet<i64>) -> ProcessRow {
    let id = entries.first().map(ChatItem::id).unwrap_or_default();
    ProcessRow {
        id,
        header: copy::process_header(entries.len()),
        entries: entries
            .iter()
            .map(|item| match item {
                ChatItem::Progress { text, ok, .. } => ProgressEntry {
                    text: text.clone(),
                    ok: *ok,
                },
                other => ProgressEntry {
                    text: format!("{other:?}"),
                    ok: None,
                },
            })
            .collect(),
        expanded: expanded.contains(&id),
    }
}

#[cfg(test)]
mod skill_draft_tests {
    use super::ChatModel;
    use xiaoman_api::SkillAction;
    use xiaoman_app::session::{PendingSkillDraft, SessionSnapshot};

    #[test]
    fn apply_snapshot_keeps_the_skill_draft_and_finish_clears_it() {
        let snapshot = SessionSnapshot {
            skill_draft: Some(PendingSkillDraft {
                id: 7,
                action: SkillAction::Create,
                name: "daily-review".to_string(),
                content: "---
name: daily-review
---

正文
"
                .to_string(),
                previous: None,
                shadows_bundled: false,
            }),
            ..SessionSnapshot::default()
        };
        let mut model = ChatModel::new();
        model.apply_snapshot(&snapshot);

        let draft = model.skill_draft().expect("draft");
        assert_eq!(draft.id, 7);
        assert_eq!(draft.name, "daily-review");
        assert!(draft.content.contains("正文"));

        model.finish();
        assert!(model.skill_draft().is_none());
    }
}
