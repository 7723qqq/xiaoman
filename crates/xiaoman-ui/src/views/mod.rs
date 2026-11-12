//! The GPUI views, one module per Compose screen plus the shared pieces.
//!
//! Every module here lays rows out and routes clicks; the rows themselves come from
//! [`crate::view_model`]. `text_field` is the one piece of real machinery: GPUI 0.2 has no text
//! input widget, so a single-line field is built from `EntityInputHandler` + `Window::handle_input`.

pub mod chat;
pub mod components;
pub mod mcp;
pub mod memory;
pub mod settings;
pub mod shell;
pub mod skills;
pub mod text_field;
