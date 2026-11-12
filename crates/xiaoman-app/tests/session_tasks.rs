//! The debug bridge's entry points on the headless session: `run_phone_task` against a fake phone,
//! the `Failed` + `PhoneTaskFinished` pair of `fail`, and the reminder delegation.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tempfile::TempDir;
use xiaoman_api::{
    Action, ActionResult, AgentEvent, AppInfo, ChatRequest, ChatResponse, Device, DeviceError,
    LlmClient, LlmError, ModelConfig, ModelRole, ReminderScheduler, ScreenState,
};
use xiaoman_app::session::{LlmFactory, RecordingReminders, Session, SessionConfig, SessionEvent};
use xiaoman_app::settings::{AppSettings, ModelSettings};

// ---------------------------------------------------------------------------------------------
// Scripted operator and fake phone
// ---------------------------------------------------------------------------------------------

struct ScriptedLlm {
    config: ModelConfig,
    responses: Mutex<VecDeque<ChatResponse>>,
}

impl ScriptedLlm {
    fn new(responses: Vec<ChatResponse>) -> Arc<Self> {
        Arc::new(Self {
            config: ModelConfig::new(
                ModelRole::Operator,
                "https://example.invalid/v1",
                "scripted",
                "test-key",
            ),
            responses: Mutex::new(responses.into()),
        })
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    fn config(&self) -> &ModelConfig {
        &self.config
    }

    async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| reply("……")))
    }
}

fn reply(text: &str) -> ChatResponse {
    ChatResponse {
        text: text.to_string(),
        tool_calls: Vec::new(),
        finish_reason: Some("stop".to_string()),
        usage: None,
    }
}

fn tool_call(name: &str, arguments: &str) -> ChatResponse {
    ChatResponse {
        text: String::new(),
        tool_calls: vec![xiaoman_api::ToolCall {
            id: "call-1".to_string(),
            name: name.to_string(),
            arguments_json: arguments.to_string(),
        }],
        finish_reason: Some("tool_calls".to_string()),
        usage: None,
    }
}

fn factory(llm: Arc<ScriptedLlm>) -> LlmFactory {
    Arc::new(move |_config: &ModelConfig| llm.clone() as Arc<dyn LlmClient>)
}

struct FakePhone;

#[async_trait]
impl Device for FakePhone {
    async fn observe(&self) -> Result<ScreenState, DeviceError> {
        Ok(ScreenState::new(
            None,
            1080,
            2400,
            Some("com.example.settings".to_string()),
        ))
    }

    async fn perform(&self, _action: Action) -> ActionResult {
        ActionResult::ok("ok")
    }

    async fn installed_apps(&self) -> Vec<AppInfo> {
        Vec::new()
    }
}

fn settings() -> AppSettings {
    AppSettings {
        chat: ModelSettings {
            model: "chat".to_string(),
            api_key: "key-1234".to_string(),
            ..ModelSettings::default()
        },
        operator: ModelSettings {
            model: "operator".to_string(),
            api_key: "key-1234".to_string(),
            ..ModelSettings::default()
        },
        max_steps: 5,
        learn_from_success: false,
    }
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "condition did not hold in time"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

// ---------------------------------------------------------------------------------------------
// run_phone_task
// ---------------------------------------------------------------------------------------------

#[test]
fn run_phone_task_uses_the_given_id_and_writes_its_trace() {
    let dir = TempDir::new().unwrap();
    let llm = ScriptedLlm::new(vec![reply(
        r#"{"thought":"完成","action":{"type":"finish","success":true,"summary":"已打开设置"}}"#,
    )]);
    let session = Session::new(
        SessionConfig::new(dir.path())
            .with_llm_factory(factory(llm))
            .with_device(Arc::new(FakePhone)),
    )
    .expect("session opens");
    session.save_settings(settings()).unwrap();

    assert!(session.run_phone_task("打开设置", None, Some("r-test")));
    wait_until(|| !session.is_busy());

    let trace = dir.path().join("traces").join("r-test").join("trace.jsonl");
    let text = std::fs::read_to_string(&trace).expect("the trace file exists");
    assert!(text.contains("PhoneTaskStarted"), "{text}");
    assert!(text.contains("PhoneTaskFinished"), "{text}");
    assert!(text.contains("已打开设置"), "{text}");
}

#[test]
fn run_phone_task_without_a_model_fails_the_run() {
    let dir = TempDir::new().unwrap();
    let session = Session::new(SessionConfig::new(dir.path()).with_device(Arc::new(FakePhone)))
        .expect("session opens");
    let mut events = session.subscribe();

    assert!(!session.run_phone_task("打开设置", None, Some("r-nokey")));

    let mut kinds = Vec::new();
    while let Some(event) = events.try_recv() {
        if let SessionEvent::Agent(event) = event {
            kinds.push(event.kind().to_string());
        }
    }
    assert!(kinds.contains(&"Failed".to_string()), "{kinds:?}");
    assert!(
        kinds.contains(&"PhoneTaskFinished".to_string()),
        "{kinds:?}"
    );
    let trace = dir
        .path()
        .join("traces")
        .join("r-nokey")
        .join("trace.jsonl");
    assert!(
        trace.is_file(),
        "a rejected run still leaves a trace for --follow"
    );
}

#[test]
fn an_invalid_run_id_is_replaced_by_a_generated_one() {
    let dir = TempDir::new().unwrap();
    let session = Session::new(SessionConfig::new(dir.path()).with_device(Arc::new(FakePhone)))
        .expect("session opens");
    let mut events = session.subscribe();
    assert!(!session.run_phone_task("打开设置", None, Some("bad id with spaces")));
    let mut ids = Vec::new();
    while let Some(event) = events.try_recv() {
        if let SessionEvent::Agent(AgentEvent::Failed { task_id, .. }) = event {
            ids.push(task_id);
        }
    }
    assert_eq!(1, ids.len());
    assert!(
        xiaoman_app::run_ids::RunIds::is_valid(&ids[0]),
        "{}",
        ids[0]
    );
}

// ---------------------------------------------------------------------------------------------
// fail
// ---------------------------------------------------------------------------------------------

#[test]
fn fail_emits_the_pair_the_cli_waits_for() {
    let dir = TempDir::new().unwrap();
    let session = Session::new(SessionConfig::new(dir.path())).expect("session opens");
    let mut events = session.subscribe();

    session.fail("r-bad", "invalid run_id");

    let mut kinds = Vec::new();
    while let Some(event) = events.try_recv() {
        if let SessionEvent::Agent(event) = event {
            kinds.push(event.kind().to_string());
        }
    }
    assert_eq!(
        vec!["Failed".to_string(), "PhoneTaskFinished".to_string()],
        kinds
    );
    let trace = dir.path().join("traces").join("r-bad").join("trace.jsonl");
    assert!(trace.is_file());
}

// ---------------------------------------------------------------------------------------------
// Reminders
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct CountingReminders {
    calls: Mutex<Vec<(String, i64)>>,
}

impl ReminderScheduler for CountingReminders {
    fn schedule(&self, text: &str, at_millis: i64) {
        self.calls
            .lock()
            .unwrap()
            .push((text.to_string(), at_millis));
    }
}

#[test]
fn the_session_records_and_forwards_reminders() {
    let platform = Arc::new(CountingReminders::default());
    let recorder = RecordingReminders::wrapping(platform.clone());
    recorder.schedule("喝水", 1_800_000_000_000);

    assert_eq!(
        vec![("喝水".to_string(), 1_800_000_000_000)],
        recorder.scheduled()
    );
    assert_eq!(
        vec![("喝水".to_string(), 1_800_000_000_000)],
        platform.calls.lock().unwrap().clone()
    );
}

#[test]
fn a_session_hands_reminders_to_the_platform_scheduler() {
    let dir = TempDir::new().unwrap();
    let platform = Arc::new(CountingReminders::default());
    let llm = ScriptedLlm::new(vec![
        tool_call("reminder_set", r#"{"text":"带伞","at":"2027-01-01T08:00"}"#),
        reply("好的，提醒你带伞。"),
    ]);
    let session = Session::new(
        SessionConfig::new(dir.path())
            .with_llm_factory(factory(llm))
            .with_reminders(platform.clone()),
    )
    .expect("session opens");
    session.save_settings(settings()).unwrap();

    assert_eq!(
        xiaoman_app::session::SendOutcome::Started,
        session.send("明早八点提醒我带伞")
    );
    wait_until(|| !session.is_busy());

    let calls = platform.calls.lock().unwrap().clone();
    assert_eq!(1, calls.len(), "the platform scheduler was called once");
    assert_eq!("带伞", calls[0].0);
    assert_eq!(calls, session.scheduled_reminders());
}

struct ScreenshotPhone;

#[async_trait]
impl Device for ScreenshotPhone {
    async fn observe(&self) -> Result<ScreenState, DeviceError> {
        Ok(ScreenState::new(
            Some(vec![0x89, b'P', b'N', b'G', 1, 2, 3]),
            1080,
            2400,
            Some("com.example.settings".to_string()),
        ))
    }

    async fn perform(&self, _action: Action) -> ActionResult {
        ActionResult::ok("ok")
    }

    async fn installed_apps(&self) -> Vec<AppInfo> {
        Vec::new()
    }
}

#[test]
fn the_observed_screenshot_lands_in_the_trace() {
    let dir = TempDir::new().unwrap();
    let llm = ScriptedLlm::new(vec![reply(
        r#"{"thought":"完成","action":{"type":"finish","success":true,"summary":"好了"}}"#,
    )]);
    let session = Session::new(
        SessionConfig::new(dir.path())
            .with_llm_factory(factory(llm))
            .with_device(Arc::new(ScreenshotPhone)),
    )
    .expect("session opens");
    session.save_settings(settings()).unwrap();

    assert!(session.run_phone_task("打开设置", None, Some("r-shot")));
    wait_until(|| !session.is_busy());

    let png = std::fs::read(dir.path().join("traces").join("r-shot").join("step-0.png"))
        .expect("the screenshot is written beside trace.jsonl");
    assert_eq!(vec![0x89, b'P', b'N', b'G', 1, 2, 3], png);
}
