//! The agent: chat orchestration, the operator loop, skills and notes.

pub mod agent_tools;
pub mod chat_tools;
pub mod flow_runner;
pub mod flow_step;
pub mod json_text;
pub mod note_text;
pub mod operator_loop;
pub mod operator_reply_parser;
pub mod prompts;
pub mod reminder_times;
pub mod screen_text;
pub mod task_run;
pub mod xiaoman_agent;

pub use chat_tools::{ChatTools, SkillEditing};
pub use flow_runner::is_yes;
pub use json_text::JsonText;
pub use note_text::NoteText;
pub use operator_loop::OperatorLoop;
pub use operator_reply_parser::{ParsedReply, extract_first_object, parse, strip_fences};
pub use reminder_times::ReminderTimes;
pub use task_run::{Executed, LoopOutcome, StepConstraint, TaskRun};
pub use xiaoman_agent::{AgentDeps, XiaomanAgent};
