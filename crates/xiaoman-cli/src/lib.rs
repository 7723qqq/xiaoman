//! Xiaoman developer CLI: lint, push, run, logs and traces.
//!
//! The command line lives in [`cli`]; the command implementations are in [`commands`] and take
//! an [`adb::Adb`] so tests can run without a device. Skill validation is in [`lint`] on top of
//! the YAML layer in [`yamlsub`], which parses through `xiaoman-skills`.

pub mod adb;
pub mod cli;
pub mod commands;
pub mod lint;
pub mod logcat;
pub mod traces;
pub mod yamlsub;

pub use adb::{Adb, AdbError, AdbOutput, RealAdb, SpawnedChild};
pub use lint::{Problem, SkillInfo, lint_flow, lint_paths, lint_skill};
pub use logcat::{LogEvent, format_event, message_of, parse_line, parse_message};
pub use traces::{
    Zone, display_width, event_fields, load_trace, render_table, resolve_trace, summarize_action,
    trace_footer, truncate,
};
pub use yamlsub::{Node, Value, YamlError, parse, parse_with, split_frontmatter, to_plain};
