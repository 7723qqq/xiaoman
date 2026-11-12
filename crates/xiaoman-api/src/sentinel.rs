//! The gate every action passes before it reaches the device.

use std::sync::Arc;

use async_trait::async_trait;

use crate::device::{Action, ScreenState};
use crate::skills::Skill;

/// Everything Sentinel may look at when judging one action.
#[derive(Debug, Clone)]
pub struct GuardContext {
    pub task_id: String,
    pub goal: String,
    pub skill: Option<Arc<Skill>>,
    /// The screen the action was planned against.
    pub screen: Option<Arc<ScreenState>>,
    /// Steps already executed in this task.
    pub step_index: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow {
        reason: String,
    },
    /// Needs the user's yes before running; `prompt` is shown on the approval card.
    Ask {
        prompt: String,
        reason: String,
    },
    Deny {
        reason: String,
    },
}

impl Decision {
    pub fn allow() -> Self {
        Self::Allow {
            reason: "allowed".to_string(),
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::Allow { reason } | Self::Ask { reason, .. } | Self::Deny { reason } => reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalScope {
    #[default]
    Once,
    Task,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRequest {
    pub task_id: String,
    pub action: Action,
    pub prompt: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalAnswer {
    pub approved: bool,
    pub scope: ApprovalScope,
}

impl ApprovalAnswer {
    pub fn once(approved: bool) -> Self {
        Self {
            approved,
            scope: ApprovalScope::Once,
        }
    }
}

/// UI side of approvals: the app shows a card and suspends until the user answers.
/// An error means the card could not be shown; the sentinel treats it as "not approved".
#[async_trait]
pub trait ApprovalGateway: Send + Sync {
    async fn request(&self, request: ApprovalRequest) -> Result<ApprovalAnswer, ApprovalError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalError(pub String);

impl std::fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ApprovalError {}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditRecord {
    pub time_millis: i64,
    pub task_id: String,
    pub step_index: i32,
    pub action: Action,
    pub decision: Decision,
    pub approved: Option<bool>,
}

pub trait AuditSink: Send + Sync {
    fn record(&self, record: AuditRecord);
}

/// Policy is evaluated outside the model: a skill cannot switch it off, only add confirmation words.
#[async_trait]
pub trait Sentinel: Send + Sync {
    async fn guard(&self, context: &GuardContext, action: &Action) -> Decision;

    /// Full gate: [`Sentinel::guard`], then the approval round-trip for [`Decision::Ask`],
    /// honoring TASK-scoped approvals already given, and an audit record.
    async fn authorize(&self, context: &GuardContext, action: &Action) -> bool;
}
