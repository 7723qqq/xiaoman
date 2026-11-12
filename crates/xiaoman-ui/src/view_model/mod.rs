//! The screens as data: one module per Compose screen, plus the Chinese [`copy`].
//!
//! Nothing here imports GPUI. Each module exposes the rows its screen paints, so the layout code
//! in [`crate::views`] stays free of decisions and the decisions stay covered by
//! `tests/view_model.rs`.

pub mod chat;
pub mod copy;
pub mod mcp;
pub mod memory;
pub mod settings;
pub mod skills;
