//! The desktop face of 小满: a pure [`view_model`] layer that owns every screen's rows, plus the
//! GPUI views that draw them.
//!
//! The split mirrors the Kotlin app: `app/src/main/kotlin/dev/xiaoman/app/ui/*` are the Compose
//! screens, and this crate repeats their information hierarchy on top of GPUI. Everything that can
//! be decided without a toolkit — timeline folding, approval card visibility, settings parsing,
//! list mapping, the Chinese copy — lives in [`view_model`] and is covered by
//! `tests/view_model.rs`. The views only lay those rows out and route clicks.
//!
//! Android-only screens and services (`MainActivity`, `AgentService`, `Notifier`,
//! `OverlayController`, `RecordingDevice`, the accessibility service) are deliberately absent: this
//! is a UI port, not an Android port.

pub mod theme;
pub mod view_model;
pub mod views;
