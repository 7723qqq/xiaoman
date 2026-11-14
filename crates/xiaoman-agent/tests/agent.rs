use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoman_agent::{AgentDeps, XiaomanAgent};
use xiaoman_api::{
    Agent, AgentConfig, AgentEvent, AgentListener, AppInfo, ChatRequest, ChatResponse, Decision,
    Device, DeviceError, GuardContext, LlmClient, LlmError, MemoryError, MemoryStore, ModelConfig,
    ModelRole, NeverStop, Note, Observation, ReminderScheduler, ScreenState, SearchHit, Sentinel,
    Skill, SkillChannel, SkillLoadResult, SkillRepository, StopSignal, ToolCall, UserChannel,
};

struct ScriptedLlm {
    config: ModelConfig,
    responses: Mutex<Vec<ChatResponse>>,
    last: Mutex<ChatResponse>,
}

impl ScriptedLlm {
    fn new(responses: Vec<ChatResponse>) -> Self {
        let last = responses.last().cloned().unwrap_or(ChatResponse {
            text: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            usage: None,
        });
        Self {
            config: ModelConfig::new(ModelRole::Chat, "http://127.0.0.1", "test", "k"),
            responses: Mutex::new(responses),
            last: Mutex::new(last),
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    fn config(&self) -> &ModelConfig {
        &self.config
    }

    async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let mut responses = self.responses.lock().unwrap();
        Ok(if responses.is_empty() {
            self.last.lock().unwrap().clone()
        } else {
            responses.remove(0)
        })
    }
}

struct EmptyMemory;

#[async_trait]
impl MemoryStore for EmptyMemory {
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

struct OneSkill;

#[async_trait]
impl SkillRepository for OneSkill {
    async fn reload(&self) -> SkillLoadResult {
        SkillLoadResult {
            skills: self.list().await,
            problems: Vec::new(),
        }
    }

    async fn list(&self) -> Vec<Skill> {
        vec![Skill {
            name: "taxi-home".into(),
            description: "打车回家".into(),
            apps: Vec::new(),
            channel: SkillChannel::Gui,
            confirm_before: Vec::new(),
            memory_refs: Vec::new(),
            deeplinks: Vec::new(),
            instructions: String::new(),
            flow: None,
            source: "test".into(),
        }]
    }

    async fn get(&self, name: &str) -> Option<Skill> {
        self.list()
            .await
            .into_iter()
            .find(|skill| skill.name == name)
    }
}

struct AllowSentinel;

#[async_trait]
impl Sentinel for AllowSentinel {
    async fn guard(&self, _context: &GuardContext, _action: &xiaoman_api::Action) -> Decision {
        Decision::allow()
    }

    async fn authorize(&self, _context: &GuardContext, _action: &xiaoman_api::Action) -> bool {
        true
    }
}

struct SilentUsers;

#[async_trait]
impl UserChannel for SilentUsers {
    async fn ask(&self, _task_id: &str, _question: &str, _takeover: bool) -> String {
        "好".to_string()
    }
}

struct StubDevice;

#[async_trait]
impl Device for StubDevice {
    async fn observe(&self) -> Result<ScreenState, DeviceError> {
        Err(DeviceError("no device".into()))
    }

    async fn perform(&self, _action: xiaoman_api::Action) -> xiaoman_api::ActionResult {
        xiaoman_api::ActionResult::failed("no device")
    }

    async fn installed_apps(&self) -> Vec<AppInfo> {
        Vec::new()
    }
}

struct NoReminders;

impl ReminderScheduler for NoReminders {
    fn schedule(&self, _text: &str, _at_millis: i64) {}
}

#[derive(Default)]
struct Recorder(Mutex<Vec<AgentEvent>>);

impl AgentListener for Recorder {
    fn on_event(&self, event: &AgentEvent) {
        self.0.lock().unwrap().push(event.clone());
    }
}

fn agent(responses: Vec<ChatResponse>) -> (XiaomanAgent, Arc<Recorder>) {
    let recorder = Arc::new(Recorder::default());
    let agent = XiaomanAgent::new(
        AgentDeps {
            chat: Arc::new(ScriptedLlm::new(responses)),
            operator: Arc::new(ScriptedLlm::new(Vec::new())),
            device: Arc::new(StubDevice),
            memory: Arc::new(EmptyMemory),
            skills: Arc::new(OneSkill),
            sentinel: Arc::new(AllowSentinel),
            users: Arc::new(SilentUsers),
            reminders: Arc::new(NoReminders),
            skill_editing: None,
            vocab: None,
        },
        AgentConfig::default(),
    );
    (agent, recorder)
}

fn reply(text: &str) -> ChatResponse {
    ChatResponse {
        text: text.to_string(),
        tool_calls: Vec::new(),
        finish_reason: None,
        usage: None,
    }
}

fn stop() -> Arc<dyn StopSignal> {
    Arc::new(NeverStop)
}

#[test]
fn plain_reply_is_emitted_once() {
    let (agent, recorder) = agent(vec![reply("你好")]);
    let listener: Arc<dyn AgentListener> = recorder.clone();
    pollster::block_on(agent.handle("你好", listener, stop()));

    let events = recorder.0.lock().unwrap().clone();
    assert_eq!(events.len(), 1, "{events:?}");
    match &events[0] {
        AgentEvent::Reply { text, .. } => assert_eq!(text, "你好"),
        other => panic!("expected reply, got {other:?}"),
    }
}

#[test]
fn tool_call_round_trips_before_the_final_reply() {
    let call = ToolCall {
        id: "call_1".into(),
        name: "list_skills".into(),
        arguments_json: "{}".into(),
    };
    let with_call = ChatResponse {
        text: String::new(),
        tool_calls: vec![call.clone()],
        finish_reason: Some("tool_calls".into()),
        usage: None,
    };
    let (agent, recorder) = agent(vec![with_call, reply("有一个技能")]);
    let listener: Arc<dyn AgentListener> = recorder.clone();
    pollster::block_on(agent.handle("有哪些技能", listener, stop()));

    let events = recorder.0.lock().unwrap().clone();
    assert_eq!(events.len(), 1, "{events:?}");
    match &events[0] {
        AgentEvent::Reply { text, .. } => assert_eq!(text, "有一个技能"),
        other => panic!("expected reply, got {other:?}"),
    }
}

#[test]
fn unknown_skill_fails_the_phone_task_without_touching_the_device() {
    let (agent, recorder) = agent(Vec::new());
    let listener: Arc<dyn AgentListener> = recorder.clone();
    let event =
        pollster::block_on(agent.run_phone_task("打开设置", Some("missing"), listener, stop()));
    match event {
        AgentEvent::PhoneTaskFinished {
            success,
            summary,
            steps,
            ..
        } => {
            assert!(!success);
            assert!(summary.contains("找不到技能"), "{summary}");
            assert_eq!(steps, 0);
        }
        other => panic!("expected phone task finished, got {other:?}"),
    }
    let events = recorder.0.lock().unwrap().clone();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::PhoneTaskStarted { .. }))
    );
}

#[test]
fn agent_is_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>(_: &T) {}
    let (agent, _) = agent(Vec::new());
    assert_send_sync(&agent);
    assert_eq!(AtomicUsize::new(0).load(Ordering::SeqCst), 0);
}
