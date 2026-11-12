//! The Android accessibility layer, ported from the Kotlin `hands/` module.
//!
//! * [`device`] — the [`xiaoman_api::Device`] over the [`bridge::AccessibilityHost`] traits.
//! * [`node_compactor`] — the compact node list handed to the operator model.
//! * [`geometry`] — pixel/normalized conversion, screenshot downscaling and the tap/swipe helpers.
//! * [`app_links`] — the `xiaoman://` links that become system intents.
//! * [`stop_signal`] — the cooperative stop flag of the overlay's Stop button.
//! * [`bridge`] — the written JNI contract with the two Java classes.
//! * `jni_bridge` — the JNI implementation of those traits, built for Android only.
//! * [`jni_exports`] — the `Java_dev_xiaoman_*` entry points Kotlin calls back into, Android only.
//! * `smoke` — the scripted see-and-touch run `adb shell am broadcast -a dev.xiaoman.smoke`
//!   starts, Android only.

pub mod app_links;
pub mod bridge;
pub mod device;
pub mod geometry;
pub mod node_compactor;
pub mod stop_signal;

#[cfg(target_os = "android")]
pub mod jni_bridge;

#[cfg(target_os = "android")]
pub mod jni_exports;

#[cfg(target_os = "android")]
pub mod smoke;

#[cfg(target_os = "android")]
pub mod secret_store;

#[cfg(target_os = "android")]
pub mod session;

pub use app_links::{AlarmLink, AppLink, AppLinkError, link_for};
pub use device::{
    AndroidDevice, AndroidDeviceOptions, Clock, NOT_A_DEVICE_ACTION, NOT_CONNECTED, SystemClock,
};
pub use geometry::{
    PxRect, Stroke, denormalize, long_press_ms, looks_all_black, normalize_axis, normalize_rect,
    swipe_ms,
};
pub use node_compactor::{CompactNode, NodeCompactor, NodeFacts, NodeTree, description_or_hint};
pub use stop_signal::MutableStopSignal;
