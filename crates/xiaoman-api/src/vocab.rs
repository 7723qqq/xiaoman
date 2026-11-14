//! The vocabulary learning engine: word cards, the review schedule and the user's preferences.
//!
//! The engine owns all persistent state; the chat model only reads today's plan and reports the
//! answers. Dates are `YYYY-MM-DD` strings in the device's local timezone, so this module stays
//! independent of a date-time library.

use serde::{Deserialize, Serialize};

use crate::ReminderScheduler;

/// One English-Chinese collocation shown under a word.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabCollocation {
    pub en: String,
    pub zh: String,
}

/// A word card. `example`, `example_zh` and `collocations` may be empty for bundled words; the
/// chat model fills them when it renders a card.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabWord {
    pub word: String,
    #[serde(default)]
    pub phonetic: String,
    #[serde(default)]
    pub pos: String,
    pub meaning: String,
    #[serde(default)]
    pub example: String,
    #[serde(default)]
    pub example_zh: String,
    #[serde(default)]
    pub collocations: Vec<VocabCollocation>,
}

impl VocabWord {
    /// `pos` plus the meaning, unless the bundled meaning already starts with a part-of-speech
    /// label (which ECDICT meanings usually do).
    pub fn display_meaning(&self) -> String {
        let meaning = self.meaning.trim();
        let pos = self.pos.trim();
        if pos.is_empty() || meaning_starts_with_pos(meaning) {
            meaning.to_string()
        } else {
            format!("{pos} {meaning}")
        }
    }
}

fn meaning_starts_with_pos(text: &str) -> bool {
    const POS: [&str; 14] = [
        "n.", "v.", "vt.", "vi.", "adj.", "adv.", "prep.", "conj.", "pron.", "num.", "art.",
        "int.", "aux.", "modal.",
    ];
    POS.iter().any(|label| text.starts_with(label))
}

/// A word that is due for review today.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabDueWord {
    #[serde(flatten)]
    pub word: VocabWord,
    /// 0 after the first study day, 1 after the +1-day review, up to 5 when mastered.
    pub stage: u8,
    /// The day this word became due.
    pub due: String,
    pub wrong: u32,
}

/// The user's study preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabPrefs {
    pub daily_new: u32,
    /// Local time, `HH:MM`.
    pub reminder_time: String,
    pub reminder_enabled: bool,
}

impl Default for VocabPrefs {
    fn default() -> Self {
        Self {
            daily_new: 15,
            reminder_time: "20:00".to_string(),
            reminder_enabled: false,
        }
    }
}

/// Progress numbers shown in the chat and on the vocabulary page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabStats {
    pub learned: u32,
    pub mastered: u32,
    pub due_today: u32,
    pub streak_days: u32,
    pub total: u32,
    pub remaining: u32,
}

/// Everything one study day needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabToday {
    pub date: String,
    pub new_words: Vec<VocabWord>,
    pub due_words: Vec<VocabDueWord>,
    /// Due words across all batches; `due_words` is capped to keep one turn manageable.
    pub due_total: u32,
    pub remaining: u32,
    pub prefs: VocabPrefs,
    pub stats: VocabStats,
}

/// One answer reported after a review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabAnswer {
    pub word: String,
    pub correct: bool,
}

/// One answer the engine accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabGradedWord {
    pub word: String,
    pub correct: bool,
    pub stage: u8,
    /// `None` once the word is mastered.
    pub next_due: Option<String>,
}

/// One answer the engine refused, with a reason the model can relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabRejectedWord {
    pub word: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VocabGradeOutcome {
    pub applied: Vec<VocabGradedWord>,
    pub rejected: Vec<VocabRejectedWord>,
    pub stats: VocabStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VocabError(pub String);

impl std::fmt::Display for VocabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for VocabError {}

/// The vocabulary engine used by the chat tools, the sessions and the screens.
///
/// Implementations own the word book and the persistent state. All dates are local
/// `YYYY-MM-DD` strings; callers take them from the device clock.
pub trait VocabEngine: Send + Sync {
    /// Returns today's plan, creating the day's new-word batch on the first call.
    fn today(&self, today: &str) -> Result<VocabToday, VocabError>;

    /// Records review answers. Only words due on `today` are accepted.
    fn grade(&self, today: &str, answers: &[VocabAnswer]) -> Result<VocabGradeOutcome, VocabError>;

    fn prefs(&self) -> Result<VocabPrefs, VocabError>;

    /// Replaces the preferences after validating them; returns the stored values.
    fn set_prefs(&self, prefs: VocabPrefs) -> Result<VocabPrefs, VocabError>;

    /// Adds a user word (for example one the model generated) and schedules it for tomorrow.
    fn add_word(&self, today: &str, word: VocabWord) -> Result<VocabWord, VocabError>;

    fn stats(&self, today: &str) -> Result<VocabStats, VocabError>;

    /// Schedules the coming reminders, cancelling the ones whose time no longer matches.
    /// `offset_minutes` is `Some` when the host knows the user's zone (the web server); `None`
    /// means the device's local timezone.
    fn ensure_reminders(
        &self,
        now_millis: i64,
        offset_minutes: Option<i32>,
        scheduler: &dyn ReminderScheduler,
    );

    /// Cancels every stored reminder and schedules the full window again. Called when a session
    /// starts, because a device reboot clears Android's alarms.
    fn reschedule_reminders(
        &self,
        now_millis: i64,
        offset_minutes: Option<i32>,
        scheduler: &dyn ReminderScheduler,
    );
}

#[cfg(test)]
mod tests {
    use super::VocabWord;

    #[test]
    fn display_meaning_does_not_repeat_the_pos() {
        let mut word = VocabWord {
            word: "state".to_string(),
            pos: "n.".to_string(),
            meaning: "n. 州；状态".to_string(),
            ..Default::default()
        };
        assert_eq!(word.display_meaning(), "n. 州；状态");
        word.meaning = "州；状态".to_string();
        assert_eq!(word.display_meaning(), "n. 州；状态");
        word.pos.clear();
        assert_eq!(word.display_meaning(), "州；状态");
    }
}
