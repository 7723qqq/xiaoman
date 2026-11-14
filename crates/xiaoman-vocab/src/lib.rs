//! The vocabulary learning engine.
//!
//! `WordBook` holds the bundled 专升本 word list, `VocabEngineImpl` owns the learned words and
//! the Ebbinghaus schedule, and `VocabStore` persists that state. The chat tools and the screens
//! depend on `xiaoman_api::VocabEngine`, not on this crate.

pub mod book;
pub mod engine;
pub mod model;
pub mod schedule;
pub mod store;

pub use book::{WordBook, normalize_key};
pub use engine::VocabEngineImpl;
pub use model::{MAX_DUE_PER_CALL, REMINDER_HORIZON_DAYS, REMINDER_TEXT, VocabState};
pub use store::{FileVocabStore, MemoryVocabStore, VocabStore};
