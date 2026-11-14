//! The headless part of the app: settings and their stores, the chat list model, run ids and the
//! trace writer. Screens (Compose in the Kotlin app) are not part of this crate.
//!
//! Each module names the Kotlin file it mirrors; keys, defaults, validation and the
//! `trace.jsonl` wire format stay byte-compatible so traces and preferences from both versions
//! can be compared.

pub mod chat_item;
pub mod json;
pub mod mcp_settings;
pub mod model_probe;
pub mod run_ids;
pub mod session;
pub mod settings;
pub mod trace;
