use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use xiaoman_agent::ChatTools;
use xiaoman_agent::agent_tools::{
    self, VOCAB_ADD_WORD, VOCAB_GRADE, VOCAB_PREFS, VOCAB_STATUS, VOCAB_TODAY,
};
use xiaoman_api::{
    MemoryError, MemoryStore, Note, Observation, ReminderScheduler, SearchHit, Skill,
    SkillLoadResult, SkillRepository, ToolCall, VocabAnswer, VocabDueWord, VocabEngine, VocabError,
    VocabGradeOutcome, VocabGradedWord, VocabPrefs, VocabStats, VocabToday, VocabWord,
};

struct NoMemory;

#[async_trait]
impl MemoryStore for NoMemory {
    async fn write(&self, note: Note) -> Result<Note, MemoryError> {
        Ok(note)
    }

    async fn read(&self, _identifier: &str) -> Result<Option<Note>, MemoryError> {
        Ok(None)
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SearchHit>, MemoryError> {
        Ok(Vec::new())
    }

    async fn append_observation(
        &self,
        _identifier: &str,
        _observation: Observation,
    ) -> Result<Note, MemoryError> {
        Err(MemoryError("not stored".into()))
    }

    async fn delete(&self, _identifier: &str) -> Result<bool, MemoryError> {
        Ok(false)
    }

    async fn list(&self, _folder: Option<&str>) -> Result<Vec<Note>, MemoryError> {
        Ok(Vec::new())
    }
}

struct NoSkills;

#[async_trait]
impl SkillRepository for NoSkills {
    async fn reload(&self) -> SkillLoadResult {
        SkillLoadResult {
            skills: Vec::new(),
            problems: Vec::new(),
        }
    }

    async fn list(&self) -> Vec<Skill> {
        Vec::new()
    }

    async fn get(&self, _name: &str) -> Option<Skill> {
        None
    }
}

struct NoReminders;

impl ReminderScheduler for NoReminders {
    fn schedule(&self, _text: &str, _at_millis: i64) {}
}

#[derive(Default)]
struct FakeVocab {
    prefs: Mutex<VocabPrefs>,
    answers: Mutex<Vec<VocabAnswer>>,
    added: Mutex<Vec<VocabWord>>,
    ensured: AtomicUsize,
}

fn word(text: &str) -> VocabWord {
    VocabWord {
        word: text.to_string(),
        phonetic: "ˈtest".to_string(),
        pos: "n.".to_string(),
        meaning: "测试".to_string(),
        ..Default::default()
    }
}

impl VocabEngine for FakeVocab {
    fn today(&self, today: &str) -> Result<VocabToday, VocabError> {
        Ok(VocabToday {
            date: today.to_string(),
            new_words: vec![word("abandon")],
            due_words: vec![VocabDueWord {
                word: word("benefit"),
                stage: 1,
                due: today.to_string(),
                wrong: 0,
            }],
            due_total: 1,
            remaining: 100,
            prefs: self.prefs.lock().unwrap().clone(),
            stats: VocabStats {
                learned: 2,
                due_today: 1,
                total: 102,
                remaining: 100,
                ..Default::default()
            },
        })
    }

    fn grade(
        &self,
        _today: &str,
        answers: &[VocabAnswer],
    ) -> Result<VocabGradeOutcome, VocabError> {
        self.answers.lock().unwrap().extend_from_slice(answers);
        Ok(VocabGradeOutcome {
            applied: answers
                .iter()
                .map(|answer| VocabGradedWord {
                    word: answer.word.clone(),
                    correct: answer.correct,
                    stage: 2,
                    next_due: Some("2026-11-20".to_string()),
                })
                .collect(),
            rejected: Vec::new(),
            stats: VocabStats::default(),
        })
    }

    fn prefs(&self) -> Result<VocabPrefs, VocabError> {
        Ok(self.prefs.lock().unwrap().clone())
    }

    fn set_prefs(&self, prefs: VocabPrefs) -> Result<VocabPrefs, VocabError> {
        *self.prefs.lock().unwrap() = prefs.clone();
        Ok(prefs)
    }

    fn add_word(&self, _today: &str, word: VocabWord) -> Result<VocabWord, VocabError> {
        self.added.lock().unwrap().push(word.clone());
        Ok(word)
    }

    fn stats(&self, _today: &str) -> Result<VocabStats, VocabError> {
        Ok(VocabStats::default())
    }

    fn ensure_reminders(
        &self,
        _now_millis: i64,
        _offset_minutes: Option<i32>,
        _scheduler: &dyn ReminderScheduler,
    ) {
        self.ensured.fetch_add(1, Ordering::SeqCst);
    }

    fn reschedule_reminders(
        &self,
        _now_millis: i64,
        _offset_minutes: Option<i32>,
        _scheduler: &dyn ReminderScheduler,
    ) {
    }
}

fn tools(vocab: Option<Arc<FakeVocab>>) -> ChatTools {
    ChatTools {
        memory: Arc::new(NoMemory),
        skills: Arc::new(NoSkills),
        clock: Arc::new(|| 1_763_000_000_000),
        reminders: Arc::new(NoReminders),
        external: None,
        phone_tasks: false,
        skill_editing: None,
        vocab: vocab.map(|vocab| vocab as Arc<dyn VocabEngine>),
        utc_offset_minutes: None,
    }
}

fn call(tools: &ChatTools, name: &str, arguments_json: &str) -> Value {
    let call = ToolCall {
        id: "call_1".into(),
        name: name.into(),
        arguments_json: arguments_json.into(),
    };
    let text = pollster::block_on(tools.execute(&call, "task", &|_| {}));
    serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
}

#[test]
fn vocab_specs_are_offered_only_with_an_engine() {
    let names: Vec<String> = agent_tools::vocab_specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert_eq!(
        names,
        vec![
            VOCAB_TODAY,
            VOCAB_GRADE,
            VOCAB_STATUS,
            VOCAB_PREFS,
            VOCAB_ADD_WORD
        ]
    );

    let without = tools(None);
    let value = call(&without, VOCAB_TODAY, "{}");
    assert!(
        value["error"]
            .as_str()
            .unwrap_or_default()
            .contains("没有背单词引擎")
    );
}

#[test]
fn vocab_today_returns_the_engine_plan_and_refreshes_reminders() {
    let fake = Arc::new(FakeVocab::default());
    let value = call(&tools(Some(fake.clone())), VOCAB_TODAY, "{}");
    assert_eq!(value["new_words"][0]["word"], "abandon");
    assert_eq!(value["due_words"][0]["word"], "benefit");
    assert_eq!(value["due_words"][0]["stage"], 1);
    assert_eq!(value["due_total"], 1);
    assert_eq!(value["stats"]["learned"], 2);
    assert_eq!(fake.ensured.load(Ordering::SeqCst), 1);
}

#[test]
fn vocab_grade_parses_answers() {
    let fake = Arc::new(FakeVocab::default());
    let value = call(
        &tools(Some(fake.clone())),
        VOCAB_GRADE,
        r#"{"answers":[{"word":"abandon","correct":true},{"word":"benefit","correct":false}]}"#,
    );
    assert_eq!(value["applied"].as_array().unwrap().len(), 2);
    assert_eq!(value["applied"][0]["stage"], 2);
    let answers = fake.answers.lock().unwrap();
    assert_eq!(answers.len(), 2);
    assert!(answers[0].correct);
    assert!(!answers[1].correct);
}

#[test]
fn vocab_grade_rejects_a_missing_correct_flag() {
    let fake = Arc::new(FakeVocab::default());
    let value = call(
        &tools(Some(fake)),
        VOCAB_GRADE,
        r#"{"answers":[{"word":"abandon"}]}"#,
    );
    assert!(
        value["error"]
            .as_str()
            .unwrap_or_default()
            .contains("correct")
    );
}

#[test]
fn vocab_status_reads_progress_without_creating_a_batch() {
    let fake = Arc::new(FakeVocab::default());
    let value = call(&tools(Some(fake.clone())), VOCAB_STATUS, "{}");
    assert!(value["date"].is_string());
    assert_eq!(value["stats"]["learned"], 0);
    assert_eq!(value["prefs"]["daily_new"], 15);
    assert_eq!(fake.ensured.load(Ordering::SeqCst), 0);
}

#[test]
fn vocab_prefs_reads_then_writes() {
    let fake = Arc::new(FakeVocab::default());
    let tools = tools(Some(fake.clone()));
    let read = call(&tools, VOCAB_PREFS, "{}");
    assert_eq!(read["daily_new"], 15);
    assert_eq!(read["reminder_time"], "20:00");

    let written = call(
        &tools,
        VOCAB_PREFS,
        r#"{"daily_new":20,"reminder_time":"07:30","reminder_enabled":true}"#,
    );
    assert_eq!(written["daily_new"], 20);
    assert_eq!(written["reminder_time"], "07:30");
    assert_eq!(written["reminder_enabled"], true);
    assert_eq!(fake.prefs.lock().unwrap().daily_new, 20);
    assert_eq!(fake.ensured.load(Ordering::SeqCst), 2);
}

#[test]
fn vocab_add_word_parses_a_full_card() {
    let fake = Arc::new(FakeVocab::default());
    let value = call(
        &tools(Some(fake.clone())),
        VOCAB_ADD_WORD,
        r#"{"word":"resilient","meaning":"有韧性的","phonetic":"rɪˈzɪliənt","pos":"adj.","example":"She is resilient.","example_zh":"她很有韧性。","collocations":[{"en":"resilient economy","zh":"有韧性的经济"}]}"#,
    );
    assert_eq!(value["ok"], true);
    assert_eq!(value["word"], "resilient");
    let added = fake.added.lock().unwrap();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].collocations[0].zh, "有韧性的经济");
    assert_eq!(added[0].example_zh, "她很有韧性。");
}
