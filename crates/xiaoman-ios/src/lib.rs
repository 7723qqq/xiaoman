//! iPhone hands and eyes behind a running WebDriverAgent, plus the `ios-runner` command line.
//!
//! The WDA device and the Mac runner:
//!
//! - [`WdaClient`]: HTTP transport to WebDriverAgent with a lazy session and one retry after an
//!   invalid session.
//! - [`WdaSource`]: flattens the WDA page source XML into [`xiaoman_api::UiNode`]s.
//! - [`WdaDevice`]: implements [`xiaoman_api::Device`] on top of the two above.
//! - [`runner`]: the `ios-runner` CLI, its console confirmation channels and the trace recorder.

pub mod ios_platform;
pub mod runner;
pub mod wda_client;
pub mod wda_device;
pub mod wda_source;

pub use ios_platform::IosPlatform;
pub use wda_client::{WdaClient, WdaError};
pub use wda_device::{WdaDevice, WdaDeviceOptions};
pub use wda_source::{Entry, Snapshot, WdaSource, WdaSourceError};
