use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoman_agent::OperatorLoop;
use xiaoman_agent::task_run::TaskRun;
use xiaoman_api::{
    Action, ActionResult, AgentConfig, AgentEvent, AgentListener, AppInfo, ChatRequest,
    ChatResponse, Decision, Device, DeviceError, GuardContext, LlmClient, LlmError, ModelConfig,
    ModelRole, NeverStop, ScreenState, Sentinel, UiNode, UserChannel,
};

struct ScriptedLlm {
    config: ModelConfig,
    replies: Mutex<Vec<String>>,
    last: Mutex<String>,
    calls: AtomicUsize,
}

impl ScriptedLlm {
    fn new(replies: &[&str]) -> Self {
        let last = replies
            .last()
            .map(|reply| (*reply).to_string())
            .unwrap_or_default();
        Self {
            config: ModelConfig::new(ModelRole::Operator, "http://127.0.0.1", "test", "k"),
            replies: Mutex::new(replies.iter().map(|reply| (*reply).to_string()).collect()),
            last: Mutex::new(last),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    fn config(&self) -> &ModelConfig {
        &self.config
    }

    async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut replies = self.replies.lock().unwrap();
        let text = if replies.is_empty() {
            self.last.lock().unwrap().clone()
        } else {
            replies.remove(0)
        };
        Ok(ChatResponse {
            text,
            tool_calls: Vec::new(),
            finish_reason: None,
            usage: None,
        })
    }
}

struct FakeDevice {
    screen: ScreenState,
    performs: AtomicUsize,
}

impl FakeDevice {
    fn new() -> Self {
        let mut screen = ScreenState::new(None, 1080, 2400, Some("com.example".into()));
        screen.nodes = vec![UiNode {
            id: 0,
            text: Some("首页".into()),
            description: None,
            resource_id: None,
            class_name: None,
            bounds: xiaoman_api::Bounds {
                left: 0,
                top: 0,
                right: 100,
                bottom: 100,
            },
            clickable: true,
            editable: false,
            scrollable: false,
            focused: false,
            password: false,
        }];
        Self {
            screen,
            performs: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Device for FakeDevice {
    async fn observe(&self) -> Result<ScreenState, DeviceError> {
        Ok(self.screen.clone())
    }

    async fn perform(&self, _action: Action) -> ActionResult {
        self.performs.fetch_add(1, Ordering::SeqCst);
        ActionResult::ok("done")
    }

    async fn installed_apps(&self) -> Vec<AppInfo> {
        Vec::new()
    }
}

struct AllowSentinel;

#[async_trait]
impl Sentinel for AllowSentinel {
    async fn guard(&self, _context: &GuardContext, _action: &Action) -> Decision {
        Decision::allow()
    }

    async fn authorize(&self, _context: &GuardContext, _action: &Action) -> bool {
        true
    }
}

struct DenySentinel;

#[async_trait]
impl Sentinel for DenySentinel {
    async fn guard(&self, _context: &GuardContext, _action: &Action) -> Decision {
        Decision::Deny {
            reason: "not allowed".into(),
        }
    }

    async fn authorize(&self, _context: &GuardContext, _action: &Action) -> bool {
        false
    }
}

struct SilentUsers;

#[async_trait]
impl UserChannel for SilentUsers {
    async fn ask(&self, _task_id: &str, _question: &str, _takeover: bool) -> String {
        "好".to_string()
    }
}

fn config() -> AgentConfig {
    AgentConfig {
        max_steps: 10,
        stuck_repeats: 3,
        ..Default::default()
    }
}

fn task(id: &str) -> TaskRun {
    let listener: Arc<dyn AgentListener> = Arc::new(|_: &AgentEvent| {});
    TaskRun::new(id, "打开设置", None, listener, Arc::new(NeverStop))
}

#[test]
fn finish_ends_the_loop() {
    let llm = Arc::new(ScriptedLlm::new(&[
        r#"{"action":{"type":"finish","success":true,"summary":"完成"}}"#,
    ]));
    let device = Arc::new(FakeDevice::new());
    let loop_ = OperatorLoop::new(
        llm,
        device.clone(),
        Arc::new(AllowSentinel),
        Arc::new(SilentUsers),
        config(),
    );
    let mut task = task("t1");

    let outcome = pollster::block_on(loop_.run(&mut task, None, None, 10));
    assert!(outcome.success);
    assert_eq!(outcome.summary, "完成");
    assert_eq!(device.performs.load(Ordering::SeqCst), 0);
}

#[test]
fn stuck_screen_warns_once_then_fails() {
    let llm = Arc::new(ScriptedLlm::new(&[r#"{"action":{"type":"back"}}"#]));
    let device = Arc::new(FakeDevice::new());
    let loop_ = OperatorLoop::new(
        llm,
        device,
        Arc::new(AllowSentinel),
        Arc::new(SilentUsers),
        config(),
    );
    let mut task = task("t2");

    let outcome = pollster::block_on(loop_.run(&mut task, None, None, 10));
    assert!(!outcome.success);
    assert_eq!(outcome.summary, xiaoman_agent::prompts::STUCK_FAILED);
}

#[test]
fn blocked_actions_never_reach_the_device() {
    let llm = Arc::new(ScriptedLlm::new(&[
        r#"{"action":{"type":"back"}}"#,
        r#"{"action":{"type":"finish","success":true,"summary":"换个做法完成"}}"#,
    ]));
    let device = Arc::new(FakeDevice::new());
    let loop_ = OperatorLoop::new(
        llm,
        device.clone(),
        Arc::new(DenySentinel),
        Arc::new(SilentUsers),
        config(),
    );
    let mut task = task("t3");

    let outcome = pollster::block_on(loop_.run(&mut task, None, None, 10));
    assert!(outcome.success);
    assert_eq!(outcome.summary, "换个做法完成");
    assert_eq!(device.performs.load(Ordering::SeqCst), 0);
    assert_eq!(task.steps, 0);
}

#[test]
fn successful_action_counts_a_step() {
    let llm = Arc::new(ScriptedLlm::new(&[
        r#"{"action":{"type":"back"}}"#,
        r#"{"action":{"type":"finish","success":true,"summary":"完成"}}"#,
    ]));
    let device = Arc::new(FakeDevice::new());
    let loop_ = OperatorLoop::new(
        llm,
        device.clone(),
        Arc::new(AllowSentinel),
        Arc::new(SilentUsers),
        config(),
    );
    let mut task = task("t4");

    let outcome = pollster::block_on(loop_.run(&mut task, None, None, 10));
    assert!(outcome.success);
    assert_eq!(device.performs.load(Ordering::SeqCst), 1);
    assert_eq!(task.steps, 1);
}
