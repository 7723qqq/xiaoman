use std::sync::{Arc, Mutex};

use xiaoman_api::{ReminderScheduler, VocabAnswer, VocabEngine, VocabError, VocabPrefs, VocabWord};
use xiaoman_vocab::{FileVocabStore, MemoryVocabStore, VocabEngineImpl, VocabStore, WordBook};

const BOOK: &str = r#"{"words":[
  {"word":"alpha","phonetic":"ˈælfə","pos":"n.","meaning":"第一个"},
  {"word":"beta","phonetic":"ˈbiːtə","pos":"n.","meaning":"第二个"},
  {"word":"gamma","phonetic":"ˈɡæmə","pos":"n.","meaning":"第三个"},
  {"word":"delta","phonetic":"ˈdeltə","pos":"n.","meaning":"第四个"},
  {"word":"epsilon","phonetic":"ˈepsɪlɒn","pos":"n.","meaning":"第五个"}
]}"#;

fn book() -> Arc<WordBook> {
    Arc::new(WordBook::parse(BOOK).unwrap())
}

fn engine() -> VocabEngineImpl {
    VocabEngineImpl::in_memory(book()).unwrap()
}

fn prefs(daily: u32, reminder: &str, enabled: bool) -> VocabPrefs {
    VocabPrefs {
        daily_new: daily,
        reminder_time: reminder.to_string(),
        reminder_enabled: enabled,
    }
}

fn words(today: &xiaoman_api::VocabToday) -> Vec<&str> {
    today
        .new_words
        .iter()
        .map(|word| word.word.as_str())
        .collect()
}

#[test]
fn today_creates_one_batch_and_reviews_are_due_the_next_day() {
    let engine = engine();
    engine.set_prefs(prefs(2, "20:00", false)).unwrap();

    let first = engine.today("2026-11-13").unwrap();
    assert_eq!(words(&first), vec!["alpha", "beta"]);
    assert_eq!(first.due_words.len(), 0);
    assert_eq!(first.remaining, 3);
    assert_eq!(first.stats.learned, 2);

    let again = engine.today("2026-11-13").unwrap();
    assert_eq!(words(&again), vec!["alpha", "beta"]);
    assert_eq!(again.stats.learned, 2);

    let next = engine.today("2026-11-14").unwrap();
    assert_eq!(words(&next), vec!["gamma", "delta"]);
    assert_eq!(next.due_total, 2);
    assert_eq!(next.due_words.len(), 2);
    assert_eq!(next.due_words[0].word.word, "alpha");
    assert_eq!(next.due_words[0].due, "2026-11-14");
    assert_eq!(next.stats.learned, 4);
}

#[test]
fn correct_reviews_follow_the_ebbinghaus_intervals() {
    let engine = engine();
    engine.set_prefs(prefs(1, "20:00", false)).unwrap();
    engine.today("2026-11-13").unwrap();

    let outcome = engine
        .grade(
            "2026-11-14",
            &[VocabAnswer {
                word: "alpha".into(),
                correct: true,
            }],
        )
        .unwrap();
    assert_eq!(outcome.applied.len(), 1);
    assert_eq!(outcome.applied[0].stage, 1);
    assert_eq!(outcome.applied[0].next_due.as_deref(), Some("2026-11-16"));

    let outcome = engine
        .grade(
            "2026-11-16",
            &[VocabAnswer {
                word: "alpha".into(),
                correct: true,
            }],
        )
        .unwrap();
    assert_eq!(outcome.applied[0].stage, 2);
    assert_eq!(outcome.applied[0].next_due.as_deref(), Some("2026-11-20"));

    let outcome = engine
        .grade(
            "2026-11-20",
            &[VocabAnswer {
                word: "alpha".into(),
                correct: true,
            }],
        )
        .unwrap();
    assert_eq!(outcome.applied[0].stage, 3);
    assert_eq!(outcome.applied[0].next_due.as_deref(), Some("2026-11-27"));

    let outcome = engine
        .grade(
            "2026-11-27",
            &[VocabAnswer {
                word: "alpha".into(),
                correct: true,
            }],
        )
        .unwrap();
    assert_eq!(outcome.applied[0].stage, 4);
    assert_eq!(outcome.applied[0].next_due.as_deref(), Some("2026-12-12"));

    let outcome = engine
        .grade(
            "2026-12-12",
            &[VocabAnswer {
                word: "alpha".into(),
                correct: true,
            }],
        )
        .unwrap();
    assert_eq!(outcome.applied[0].stage, 5);
    assert_eq!(outcome.applied[0].next_due, None);
    assert_eq!(outcome.stats.mastered, 1);

    let later = engine.grade(
        "2026-12-13",
        &[VocabAnswer {
            word: "alpha".into(),
            correct: true,
        }],
    );
    assert_eq!(later.unwrap().rejected.len(), 1);
}

#[test]
fn wrong_answers_reset_to_tomorrow() {
    let engine = engine();
    engine.set_prefs(prefs(1, "20:00", false)).unwrap();
    engine.today("2026-11-13").unwrap();

    let outcome = engine
        .grade(
            "2026-11-14",
            &[VocabAnswer {
                word: "alpha".into(),
                correct: false,
            }],
        )
        .unwrap();
    assert_eq!(outcome.applied[0].stage, 0);
    assert_eq!(outcome.applied[0].next_due.as_deref(), Some("2026-11-15"));

    let today = engine.today("2026-11-15").unwrap();
    assert_eq!(today.due_words[0].wrong, 1);
    assert_eq!(today.due_words[0].due, "2026-11-15");

    let outcome = engine
        .grade(
            "2026-11-15",
            &[VocabAnswer {
                word: "alpha".into(),
                correct: true,
            }],
        )
        .unwrap();
    assert_eq!(outcome.applied[0].stage, 1);
    assert_eq!(outcome.applied[0].next_due.as_deref(), Some("2026-11-17"));
}

#[test]
fn an_answer_is_rejected_before_its_due_date() {
    let engine = engine();
    engine.set_prefs(prefs(1, "20:00", false)).unwrap();
    engine.today("2026-11-13").unwrap();

    let outcome = engine
        .grade(
            "2026-11-13",
            &[VocabAnswer {
                word: "alpha".into(),
                correct: true,
            }],
        )
        .unwrap();
    assert!(outcome.applied.is_empty());
    assert_eq!(outcome.rejected.len(), 1);
    assert!(outcome.rejected[0].reason.contains("还没到复习日期"));

    let outcome = engine
        .grade(
            "2026-11-14",
            &[VocabAnswer {
                word: "nope".into(),
                correct: true,
            }],
        )
        .unwrap();
    assert!(outcome.applied.is_empty());
    assert!(outcome.rejected[0].reason.contains("还没有学过"));
}

#[test]
fn due_words_are_capped_but_counted() {
    let mut words = Vec::new();
    for index in 0..35 {
        let suffix = format!(
            "{}{}",
            (b'a' + (index / 26) as u8) as char,
            (b'a' + (index % 26) as u8) as char
        );
        words.push(format!(
            r#"{{"word":"word{suffix}","meaning":"释义{index}"}}"#
        ));
    }
    let json = format!(r#"{{"words":[{}]}}"#, words.join(","));
    let engine = VocabEngineImpl::in_memory(Arc::new(WordBook::parse(&json).unwrap())).unwrap();
    engine.set_prefs(prefs(35, "20:00", false)).unwrap();
    engine.today("2026-11-13").unwrap();

    let next = engine.today("2026-11-14").unwrap();
    assert_eq!(next.due_total, 35);
    assert_eq!(next.due_words.len(), xiaoman_vocab::MAX_DUE_PER_CALL);
}

#[test]
fn zero_new_words_only_reviews_and_creates_no_batch() {
    let engine = engine();
    engine.set_prefs(prefs(0, "20:00", false)).unwrap();
    let today = engine.today("2026-11-13").unwrap();
    assert!(today.new_words.is_empty());
    assert_eq!(today.stats.learned, 0);

    engine.set_prefs(prefs(2, "20:00", false)).unwrap();
    let today = engine.today("2026-11-13").unwrap();
    assert_eq!(words(&today), vec!["alpha", "beta"]);
}

#[test]
fn custom_words_come_first_and_can_join_todays_batch() {
    let engine = engine();
    engine.set_prefs(prefs(2, "20:00", false)).unwrap();
    let added = engine
        .add_word(
            "2026-11-13",
            VocabWord {
                word: "Zeta".into(),
                meaning: "第六个".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(added.word, "Zeta");

    let today = engine.today("2026-11-13").unwrap();
    assert_eq!(words(&today), vec!["Zeta", "alpha"]);

    let duplicate = engine.add_word(
        "2026-11-13",
        VocabWord {
            word: "zeta".into(),
            meaning: "重复".into(),
            ..Default::default()
        },
    );
    assert!(duplicate.is_err());
    let bundled = engine.add_word(
        "2026-11-13",
        VocabWord {
            word: "beta".into(),
            meaning: "重复".into(),
            ..Default::default()
        },
    );
    assert!(bundled.is_err());
}

#[test]
fn preferences_are_validated_and_normalized() {
    let engine = engine();
    assert_eq!(engine.prefs().unwrap().daily_new, 15);
    assert!(engine.set_prefs(prefs(101, "20:00", false)).is_err());
    assert!(engine.set_prefs(prefs(10, "24:00", false)).is_err());

    let stored = engine.set_prefs(prefs(10, "8:05", true)).unwrap();
    assert_eq!(stored.reminder_time, "08:05");
    assert!(stored.reminder_enabled);
    assert_eq!(engine.prefs().unwrap(), stored);
}

#[test]
fn streak_counts_consecutive_study_days() {
    let engine = engine();
    engine.set_prefs(prefs(1, "20:00", false)).unwrap();
    engine.today("2026-11-13").unwrap();
    engine
        .grade(
            "2026-11-14",
            &[VocabAnswer {
                word: "alpha".into(),
                correct: true,
            }],
        )
        .unwrap();
    engine.today("2026-11-15").unwrap();

    let stats = engine.stats("2026-11-15").unwrap();
    assert_eq!(stats.streak_days, 3);
}

#[derive(Default)]
struct FakeScheduler {
    scheduled: Mutex<Vec<(String, i64)>>,
    cancelled: Mutex<Vec<(String, i64)>>,
}

impl FakeScheduler {
    fn scheduled(&self) -> Vec<(String, i64)> {
        self.scheduled.lock().unwrap().clone()
    }

    fn cancelled(&self) -> Vec<(String, i64)> {
        self.cancelled.lock().unwrap().clone()
    }
}

impl ReminderScheduler for FakeScheduler {
    fn schedule(&self, text: &str, at_millis: i64) {
        self.scheduled
            .lock()
            .unwrap()
            .push((text.to_string(), at_millis));
    }

    fn cancel(&self, text: &str, at_millis: i64) {
        self.cancelled
            .lock()
            .unwrap()
            .push((text.to_string(), at_millis));
    }
}

#[test]
fn reminders_roll_forward_and_are_cancelled_when_the_time_changes() {
    let engine = engine();
    let date = xiaoman_vocab::schedule::parse_date("2026-11-13").unwrap();
    let now = xiaoman_vocab::schedule::local_millis(date, 19, 0, None).unwrap();
    engine.set_prefs(prefs(5, "20:00", true)).unwrap();

    let scheduler = FakeScheduler::default();
    engine.ensure_reminders(now, None, &scheduler);
    assert_eq!(scheduler.scheduled().len(), 14);
    assert_eq!(
        scheduler.scheduled()[0].1,
        xiaoman_vocab::schedule::local_millis(date, 20, 0, None).unwrap()
    );

    engine.ensure_reminders(now, None, &scheduler);
    assert_eq!(scheduler.scheduled().len(), 14);

    engine.set_prefs(prefs(5, "21:00", true)).unwrap();
    engine.ensure_reminders(now, None, &scheduler);
    assert_eq!(scheduler.cancelled().len(), 14);
    assert_eq!(scheduler.scheduled().len(), 28);

    engine.set_prefs(prefs(5, "21:00", false)).unwrap();
    engine.ensure_reminders(now, None, &scheduler);
    assert_eq!(scheduler.cancelled().len(), 28);
    assert_eq!(scheduler.scheduled().len(), 28);
}

#[test]
fn reopening_a_file_engine_keeps_todays_batch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vocab/state.json");
    let store = Arc::new(FileVocabStore::new(&path));
    {
        let engine = VocabEngineImpl::open(book(), store.clone()).unwrap();
        engine.set_prefs(prefs(2, "20:00", false)).unwrap();
        let today = engine.today("2026-11-13").unwrap();
        assert_eq!(words(&today), vec!["alpha", "beta"]);
    }
    assert!(path.exists(), "state file should be written");

    let engine = VocabEngineImpl::open(book(), store).unwrap();
    let today = engine.today("2026-11-13").unwrap();
    assert_eq!(words(&today), vec!["alpha", "beta"]);
    assert_eq!(today.stats.learned, 2);

    let memory = MemoryVocabStore::new();
    let engine = VocabEngineImpl::open(book(), Arc::new(memory)).unwrap();
    assert!(engine.today("2026-11-13").is_ok());
}

#[test]
fn a_corrupt_state_is_moved_aside() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, "not json").unwrap();
    let store = FileVocabStore::new(&path);
    assert!(store.load().unwrap().is_none());
    assert!(!path.exists());
    let backups: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(backups.iter().any(|name| name.contains("corrupt")));
}

#[test]
fn a_newer_state_version_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, r#"{"version":99}"#).unwrap();
    let result = VocabEngineImpl::open(book(), Arc::new(FileVocabStore::new(&path)));
    assert!(matches!(result, Err(VocabError(message)) if message.contains("版本")));
}
