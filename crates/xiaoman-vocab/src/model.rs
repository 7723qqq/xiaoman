//! The persistent learning state: preferences, per-word progress and the daily batches.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use xiaoman_api::{VocabPrefs, VocabWord};

pub const STATE_VERSION: u32 = 1;
/// One turn shows at most this many due words; the rest stay due for the next call.
pub const MAX_DUE_PER_CALL: usize = 30;
/// How many daily reminders are kept scheduled ahead of time.
pub const REMINDER_HORIZON_DAYS: i64 = 14;
/// The fixed notification text, so scheduling the same day twice collapses on Android.
pub const REMINDER_TEXT: &str = "背单词时间到了：打开小满说『背单词』";

/// One word's place in the schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub word: String,
    pub introduced: String,
    /// 0..=5; 5 means the +15-day review passed and the word is mastered.
    pub stage: u8,
    /// The day this word becomes due again; empty once mastered.
    #[serde(default)]
    pub next_due: String,
    #[serde(default)]
    pub correct: u32,
    #[serde(default)]
    pub wrong: u32,
    #[serde(default)]
    pub last_review: Option<String>,
}

impl Progress {
    pub fn new(word: &str, introduced: &str, next_due: &str) -> Self {
        Self {
            word: word.to_string(),
            introduced: introduced.to_string(),
            stage: 0,
            next_due: next_due.to_string(),
            correct: 0,
            wrong: 0,
            last_review: None,
        }
    }
}

/// The whole engine state, serialized as `state.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VocabState {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub prefs: VocabPrefs,
    /// Keyed by the lowercased word.
    #[serde(default)]
    pub progress: BTreeMap<String, Progress>,
    /// Date -> the words introduced that day.
    #[serde(default)]
    pub batches: BTreeMap<String, Vec<String>>,
    /// Words added by the user, including model-generated cards.
    #[serde(default)]
    pub custom: Vec<VocabWord>,
    /// Days with a created batch or an accepted answer.
    #[serde(default)]
    pub study_days: BTreeSet<String>,
    /// Date -> the epoch millis of the reminder scheduled for it.
    #[serde(default)]
    pub scheduled: BTreeMap<String, i64>,
}

fn default_version() -> u32 {
    STATE_VERSION
}

impl Default for VocabState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            prefs: VocabPrefs::default(),
            progress: BTreeMap::new(),
            batches: BTreeMap::new(),
            custom: Vec::new(),
            study_days: BTreeSet::new(),
            scheduled: BTreeMap::new(),
        }
    }
}
