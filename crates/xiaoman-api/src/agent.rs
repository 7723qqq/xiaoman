//! Events streamed from the agent to the UI and the trace, plus the agent entry points.
use std::sync::Arc;

use async_trait::async_trait;

use crate::device::{Action, ActionResult};
use crate::sentinel::Decision;

/// Events streamed from the agent to the UI and the trace.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// Assistant text for the chat.
    Reply {
        task_id: String,
        text: String,
    },
    SkillChosen {
        task_id: String,
        skill_name: String,
    },
    PhoneTaskStarted {
        task_id: String,
        goal: String,
        skill_name: Option<String>,
    },
    StepPlanned {
        task_id: String,
        step_index: i32,
        action: Action,
    },
    StepExecuted {
        task_id: String,
        step_index: i32,
        action: Action,
        result: ActionResult,
    },
    StepBlocked {
        task_id: String,
        step_index: i32,
        action: Action,
        decision: Decision,
    },
    /// The loop paused for the user: a question or a takeover (login, captcha, secure page).
    NeedsUser {
        task_id: String,
        question: String,
        takeover: bool,
    },
    MemoryUpdated {
        task_id: String,
        permalink: String,
    },
    /// A tool from a [`crate::ToolProvider`] ran; `summary` is short and never contains secrets.
    ToolCalled {
        task_id: String,
        tool: String,
        ok: bool,
        summary: String,
    },
    PhoneTaskFinished {
        task_id: String,
        success: bool,
        summary: String,
        steps: i32,
    },
    Failed {
        task_id: String,
        message: String,
    },
}

impl AgentEvent {
    pub fn task_id(&self) -> &str {
        match self {
            Self::Reply { task_id, .. }
            | Self::SkillChosen { task_id, .. }
            | Self::PhoneTaskStarted { task_id, .. }
            | Self::StepPlanned { task_id, .. }
            | Self::StepExecuted { task_id, .. }
            | Self::StepBlocked { task_id, .. }
            | Self::NeedsUser { task_id, .. }
            | Self::MemoryUpdated { task_id, .. }
            | Self::ToolCalled { task_id, .. }
            | Self::PhoneTaskFinished { task_id, .. }
            | Self::Failed { task_id, .. } => task_id,
        }
    }

    /// Trace kind: the Kotlin class name, so traces from both versions line up.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Reply { .. } => "Reply",
            Self::SkillChosen { .. } => "SkillChosen",
            Self::PhoneTaskStarted { .. } => "PhoneTaskStarted",
            Self::StepPlanned { .. } => "StepPlanned",
            Self::StepExecuted { .. } => "StepExecuted",
            Self::StepBlocked { .. } => "StepBlocked",
            Self::NeedsUser { .. } => "NeedsUser",
            Self::MemoryUpdated { .. } => "MemoryUpdated",
            Self::ToolCalled { .. } => "ToolCalled",
            Self::PhoneTaskFinished { .. } => "PhoneTaskFinished",
            Self::Failed { .. } => "Failed",
        }
    }
}

pub trait AgentListener: Send + Sync {
    fn on_event(&self, event: &AgentEvent);
}

impl<F> AgentListener for F
where
    F: Fn(&AgentEvent) + Send + Sync,
{
    fn on_event(&self, event: &AgentEvent) {
        self(event);
    }
}

/// Answers [`Action::AskUser`] questions; the app routes this to the chat input.
#[async_trait]
pub trait UserChannel: Send + Sync {
    async fn ask(&self, task_id: &str, question: &str, takeover: bool) -> String;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    pub max_steps: i32,
    /// The same screen this many times in a row after acting counts as stuck.
    pub stuck_repeats: i32,
    /// How many earlier steps are replayed to the operator model as text.
    pub history_window: i32,
    /// Max nodes of the accessibility tree sent per step.
    pub max_nodes: i32,
    /// Write a "how to" note into memory after a successful phone task.
    pub learn_from_success: bool,
    /// Platform notes appended to the operator system prompt, e.g. how back and launch work on iOS.
    pub operator_notes: String,
    /// Offer `phone_task` to the chat model. Off where there is no phone to operate.
    pub phone_tasks: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_steps: 30,
            stuck_repeats: 3,
            history_window: 8,
            max_nodes: 120,
            learn_from_success: true,
            operator_notes: String::new(),
            phone_tasks: true,
        }
    }
}

/// Cooperative cancellation from the overlay's Stop button.
pub trait StopSignal: Send + Sync {
    fn stopped(&self) -> bool;
}

/// A stop signal that never fires, for tests and one-shot runs.
pub struct NeverStop;

impl StopSignal for NeverStop {
    fn stopped(&self) -> bool {
        false
    }
}

#[async_trait]
pub trait Agent: Send + Sync {
    /// Handles one chat turn: replies, memory tools, or a phone task. Returns when the turn is done.
    async fn handle(
        &self,
        user_message: &str,
        listener: Arc<dyn AgentListener>,
        stop: Arc<dyn StopSignal>,
    );

    /// Runs one phone task directly (used by the developer CLI and skill tests).
    async fn run_phone_task(
        &self,
        goal: &str,
        skill_name: Option<&str>,
        listener: Arc<dyn AgentListener>,
        stop: Arc<dyn StopSignal>,
    ) -> AgentEvent;
}
