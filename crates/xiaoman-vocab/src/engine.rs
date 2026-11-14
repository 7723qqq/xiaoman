//! The engine: daily batches, the review schedule, preferences and reminder bookkeeping.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{Duration, NaiveDate};
use xiaoman_api::{
    ReminderScheduler, VocabAnswer, VocabDueWord, VocabEngine, VocabError, VocabGradeOutcome,
    VocabGradedWord, VocabPrefs, VocabRejectedWord, VocabStats, VocabToday, VocabWord,
};

use crate::book::{WordBook, normalize_key, validate_word};
use crate::model::{
    MAX_DUE_PER_CALL, Progress, REMINDER_HORIZON_DAYS, REMINDER_TEXT, STATE_VERSION, VocabState,
};
use crate::schedule::{
    MASTERED_STAGE, add_days, format_date, local_date, local_millis, parse_date,
    parse_reminder_time, review_result,
};
use crate::store::{MemoryVocabStore, VocabStore};

pub struct VocabEngineImpl {
    book: Arc<WordBook>,
    store: Arc<dyn VocabStore>,
    state: Mutex<VocabState>,
}

impl VocabEngineImpl {
    pub fn open(book: Arc<WordBook>, store: Arc<dyn VocabStore>) -> Result<Self, VocabError> {
        let mut state = store.load()?.unwrap_or_default();
        if state.version > STATE_VERSION {
            return Err(VocabError(format!(
                "背单词状态版本 {} 高于当前支持的 {}",
                state.version, STATE_VERSION
            )));
        }
        if state.version < STATE_VERSION {
            state.version = STATE_VERSION;
            store.save(&state)?;
        }
        Ok(Self {
            book,
            store,
            state: Mutex::new(state),
        })
    }

    pub fn in_memory(book: Arc<WordBook>) -> Result<Self, VocabError> {
        Self::open(book, Arc::new(MemoryVocabStore::new()))
    }

    fn lock(&self) -> MutexGuard<'_, VocabState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn card(&self, state: &VocabState, key: &str) -> Option<VocabWord> {
        if let Some(word) = self.book.get(key) {
            return Some(word.clone());
        }
        state
            .custom
            .iter()
            .find(|word| normalize_key(&word.word) == key)
            .cloned()
    }

    /// New words in learning order: user words first (newest first), then the bundled list.
    fn pick_new(&self, state: &VocabState, limit: usize) -> Vec<VocabWord> {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(limit);
        let mut seen = std::collections::BTreeSet::new();
        for word in state.custom.iter().rev() {
            let key = normalize_key(&word.word);
            if state.progress.contains_key(&key) || !seen.insert(key) {
                continue;
            }
            out.push(word.clone());
            if out.len() >= limit {
                return out;
            }
        }
        for word in self.book.words() {
            let key = normalize_key(&word.word);
            if state.progress.contains_key(&key) || !seen.insert(key) {
                continue;
            }
            out.push(word.clone());
            if out.len() >= limit {
                break;
            }
        }
        out
    }

    fn stats_for(&self, state: &VocabState, today: NaiveDate) -> VocabStats {
        let total = self.book.len() + state.custom.len();
        let learned = state.progress.len();
        let today_text = format_date(today);
        let mastered = state
            .progress
            .values()
            .filter(|progress| progress.stage >= MASTERED_STAGE)
            .count();
        let due_today = state
            .progress
            .values()
            .filter(|progress| is_due(progress, &today_text))
            .count();
        let mut day = today;
        if !state.study_days.contains(&today_text) {
            day -= Duration::days(1);
        }
        let mut streak = 0;
        while state.study_days.contains(&format_date(day)) {
            streak += 1;
            day -= Duration::days(1);
        }
        VocabStats {
            learned: learned as u32,
            mastered: mastered as u32,
            due_today: due_today as u32,
            streak_days: streak,
            total: total as u32,
            remaining: total.saturating_sub(learned) as u32,
        }
    }

    fn due_words(&self, state: &VocabState, today: &str) -> (Vec<VocabDueWord>, u32) {
        let mut due: Vec<(&String, &Progress)> = state
            .progress
            .iter()
            .filter(|(_, progress)| is_due(progress, today))
            .collect();
        due.sort_by(|left, right| {
            left.1
                .next_due
                .cmp(&right.1.next_due)
                .then_with(|| left.0.cmp(right.0))
        });
        let total = due.len() as u32;
        let words = due
            .into_iter()
            .take(MAX_DUE_PER_CALL)
            .filter_map(|(key, progress)| {
                Some(VocabDueWord {
                    word: self.card(state, key)?,
                    stage: progress.stage,
                    due: progress.next_due.clone(),
                    wrong: progress.wrong,
                })
            })
            .collect();
        (words, total)
    }
}

fn is_due(progress: &Progress, today: &str) -> bool {
    progress.stage < MASTERED_STAGE
        && !progress.next_due.is_empty()
        && progress.next_due.as_str() <= today
}

impl VocabEngine for VocabEngineImpl {
    fn today(&self, today: &str) -> Result<VocabToday, VocabError> {
        let today_date = parse_date(today)?;
        let mut state = self.lock();
        let mut changed = false;
        let keys = if let Some(batch) = state.batches.get(today) {
            batch.clone()
        } else {
            let picked = self.pick_new(&state, state.prefs.daily_new as usize);
            if picked.is_empty() {
                Vec::new()
            } else {
                let next_due = add_days(today_date, 1);
                let mut keys = Vec::with_capacity(picked.len());
                for word in picked {
                    let key = normalize_key(&word.word);
                    state
                        .progress
                        .entry(key.clone())
                        .or_insert_with(|| Progress::new(&word.word, today, &next_due));
                    keys.push(key);
                }
                state.batches.insert(today.to_string(), keys.clone());
                state.study_days.insert(today.to_string());
                changed = true;
                keys
            }
        };
        let new_words: Vec<VocabWord> = keys
            .iter()
            .filter_map(|key| self.card(&state, key))
            .collect();
        let (due_words, due_total) = self.due_words(&state, today);
        let stats = self.stats_for(&state, today_date);
        if changed {
            self.store.save(&state)?;
        }
        Ok(VocabToday {
            date: today.to_string(),
            new_words,
            due_words,
            due_total,
            remaining: stats.remaining,
            prefs: state.prefs.clone(),
            stats,
        })
    }

    fn grade(&self, today: &str, answers: &[VocabAnswer]) -> Result<VocabGradeOutcome, VocabError> {
        let today_date = parse_date(today)?;
        if answers.is_empty() {
            return Err(VocabError("没有要记录的单词".to_string()));
        }
        let mut state = self.lock();
        let mut applied = Vec::new();
        let mut rejected = Vec::new();
        for answer in answers {
            let key = normalize_key(&answer.word);
            let Some(progress) = state.progress.get_mut(&key) else {
                rejected.push(VocabRejectedWord {
                    word: answer.word.clone(),
                    reason: "还没有学过这个单词".to_string(),
                });
                continue;
            };
            if progress.stage >= MASTERED_STAGE {
                rejected.push(VocabRejectedWord {
                    word: answer.word.clone(),
                    reason: "这个单词已经掌握，不用再复习".to_string(),
                });
                continue;
            }
            if progress.next_due.is_empty() || progress.next_due.as_str() > today {
                rejected.push(VocabRejectedWord {
                    word: answer.word.clone(),
                    reason: format!("还没到复习日期（{}）", progress.next_due),
                });
                continue;
            }
            let (stage, next_due) = review_result(today_date, progress.stage, answer.correct);
            progress.stage = stage;
            progress.next_due = next_due.clone().unwrap_or_default();
            if answer.correct {
                progress.correct += 1;
            } else {
                progress.wrong += 1;
            }
            progress.last_review = Some(today.to_string());
            applied.push(VocabGradedWord {
                word: progress.word.clone(),
                correct: answer.correct,
                stage,
                next_due,
            });
        }
        if !applied.is_empty() {
            state.study_days.insert(today.to_string());
            self.store.save(&state)?;
        }
        Ok(VocabGradeOutcome {
            applied,
            rejected,
            stats: self.stats_for(&state, today_date),
        })
    }

    fn prefs(&self) -> Result<VocabPrefs, VocabError> {
        Ok(self.lock().prefs.clone())
    }

    fn set_prefs(&self, prefs: VocabPrefs) -> Result<VocabPrefs, VocabError> {
        if prefs.daily_new > 100 {
            return Err(VocabError("每天新词数最多 100 个".to_string()));
        }
        let (time, _, _) = parse_reminder_time(&prefs.reminder_time)?;
        let mut state = self.lock();
        state.prefs = VocabPrefs {
            daily_new: prefs.daily_new,
            reminder_time: time,
            reminder_enabled: prefs.reminder_enabled,
        };
        self.store.save(&state)?;
        Ok(state.prefs.clone())
    }

    fn add_word(&self, today: &str, word: VocabWord) -> Result<VocabWord, VocabError> {
        let today_date = parse_date(today)?;
        validate_word(&word)?;
        let key = normalize_key(&word.word);
        let mut state = self.lock();
        if self.book.contains(&key) {
            return Err(VocabError(format!(
                "词库里已经有「{}」，直接用它就行",
                word.word.trim()
            )));
        }
        if state.progress.contains_key(&key)
            || state
                .custom
                .iter()
                .any(|existing| normalize_key(&existing.word) == key)
        {
            return Err(VocabError(format!(
                "「{}」已经在背词列表里",
                word.word.trim()
            )));
        }
        let mut stored = word;
        stored.word = stored.word.trim().to_string();
        state.custom.push(stored.clone());
        // Before today's batch exists the word joins the next `today` call; after it exists it is
        // appended to today's plan and scheduled immediately.
        if let Some(batch) = state.batches.get_mut(today) {
            if !batch.contains(&key) {
                batch.push(key.clone());
            }
            state
                .progress
                .entry(key)
                .or_insert_with(|| Progress::new(&stored.word, today, &add_days(today_date, 1)));
        }
        self.store.save(&state)?;
        Ok(stored)
    }

    fn stats(&self, today: &str) -> Result<VocabStats, VocabError> {
        let today_date = parse_date(today)?;
        Ok(self.stats_for(&self.lock(), today_date))
    }

    fn ensure_reminders(
        &self,
        now_millis: i64,
        offset_minutes: Option<i32>,
        scheduler: &dyn ReminderScheduler,
    ) {
        let mut state = self.lock();
        let today = local_date(now_millis, offset_minutes);
        let today_text = format_date(today);
        let mut desired: BTreeMap<String, i64> = BTreeMap::new();
        if state.prefs.reminder_enabled {
            if let Ok((_, hour, minute)) = parse_reminder_time(&state.prefs.reminder_time) {
                for offset in 0..REMINDER_HORIZON_DAYS {
                    let date = today + Duration::days(offset);
                    if let Some(millis) = local_millis(date, hour, minute, offset_minutes) {
                        if millis > now_millis {
                            desired.insert(format_date(date), millis);
                        }
                    }
                }
            }
        }
        let stored: Vec<(String, i64)> = state
            .scheduled
            .iter()
            .map(|(date, millis)| (date.clone(), *millis))
            .collect();
        let mut changed = false;
        for (date, millis) in stored {
            if desired.get(&date) != Some(&millis) {
                scheduler.cancel(REMINDER_TEXT, millis);
                state.scheduled.remove(&date);
                changed = true;
            }
        }
        for (date, millis) in desired {
            if let std::collections::btree_map::Entry::Vacant(entry) = state.scheduled.entry(date) {
                scheduler.schedule(REMINDER_TEXT, millis);
                entry.insert(millis);
                changed = true;
            }
        }
        let stale: Vec<String> = state
            .scheduled
            .keys()
            .filter(|date| date.as_str() < today_text.as_str())
            .cloned()
            .collect();
        for date in stale {
            state.scheduled.remove(&date);
            changed = true;
        }
        if changed {
            let _ = self.store.save(&state);
        }
    }

    fn reschedule_reminders(
        &self,
        now_millis: i64,
        offset_minutes: Option<i32>,
        scheduler: &dyn ReminderScheduler,
    ) {
        {
            let mut state = self.lock();
            let stored: Vec<(String, i64)> = state
                .scheduled
                .iter()
                .map(|(date, millis)| (date.clone(), *millis))
                .collect();
            for (_, millis) in stored {
                scheduler.cancel(REMINDER_TEXT, millis);
            }
            state.scheduled.clear();
            let _ = self.store.save(&state);
        }
        self.ensure_reminders(now_millis, offset_minutes, scheduler);
    }
}
