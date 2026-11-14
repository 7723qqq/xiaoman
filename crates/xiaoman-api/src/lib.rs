//! Shared interfaces and data structures. Every other crate depends only on this one.

pub mod agent;
pub mod authoring;
pub mod device;
pub mod llm;
pub mod memory;
pub mod reminder;
pub mod sentinel;
pub mod skills;
pub mod tools;
pub mod vocab;

pub use agent::*;
pub use authoring::*;
pub use device::*;
pub use llm::*;
pub use memory::*;
pub use reminder::*;
pub use sentinel::*;
pub use skills::*;
pub use tools::*;
pub use vocab::*;

/// Milliseconds since the Unix epoch, wall-clock.
pub fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
