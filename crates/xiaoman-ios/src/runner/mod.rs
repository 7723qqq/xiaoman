//! The `ios-runner` command line: argument parsing, console channels, reminders and traces.

pub mod assembly;
pub mod channels;
pub mod command;
pub mod config;
pub mod reminders;
pub mod trace;

pub use assembly::{LlmFactory, RunnerSession, assemble, default_llm_factory};
pub use channels::ConsoleChannels;
pub use command::{CtrlCStop, Runner, install_ctrl_c};
pub use config::{
    CliError, Command, DEFAULT_BASE_URL, DEFAULT_MAX_STEPS, DEFAULT_WDA_URL, ModelSettings,
    RunnerConfig, USAGE,
};
pub use reminders::MacReminders;
pub use trace::TraceRecorder;
