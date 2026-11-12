//! State of one phone task, shared by its flow steps and every operator loop they run.

use std::collections::HashMap;
use std::sync::Arc;

use xiaoman_api::{ActionResult, AgentEvent, AgentListener, Decision, Skill, StopSignal};

pub struct TaskRun {
    pub task_id: String,
    pub goal: String,
    pub skill: Option<Arc<Skill>>,
    pub listener: Arc<dyn AgentListener>,
    pub stop: Arc<dyn StopSignal>,
    /// Device actions executed so far; also the step index of the next one.
    pub steps: i32,
    /// Labels of the successful actions, for the learning note.
    pub path: Vec<String>,
    /// Skill memory refs, rendered for prompts.
    pub references: String,
    /// Earlier successful paths (howto notes), rendered for prompts.
    pub howto: String,
    /// Placeholder values already resolved in this task.
    pub values: HashMap<String, String>,
    pub launched_package: Option<String>,
    pub last_package: Option<String>,
}

impl TaskRun {
    pub fn new(
        task_id: impl Into<String>,
        goal: impl Into<String>,
        skill: Option<Arc<Skill>>,
        listener: Arc<dyn AgentListener>,
        stop: Arc<dyn StopSignal>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            goal: goal.into(),
            skill,
            listener,
            stop,
            steps: 0,
            path: Vec::new(),
            references: String::new(),
            howto: String::new(),
            values: HashMap::new(),
            launched_package: None,
            last_package: None,
        }
    }

    pub fn emit(&self, event: AgentEvent) {
        self.listener.on_event(&event);
    }

    pub fn add_path(&mut self, label: Option<&str>) {
        let Some(label) = label.map(str::trim).filter(|label| !label.is_empty()) else {
            return;
        };
        if self.path.last().map(String::as_str) != Some(label) {
            self.path.push(label.to_string());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopOutcome {
    pub success: bool,
    pub summary: String,
}

/// Restricts a loop to one action of `kind` (`tap` or `type`) aimed at `target`; `text` overrides
/// the typed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepConstraint {
    pub kind: String,
    pub target: String,
    pub text: Option<String>,
}

/// Outcome of `OperatorLoop::execute`.
#[derive(Debug, Clone, PartialEq)]
pub enum Executed {
    Done { result: ActionResult },
    Blocked { decision: Decision },
}
