//! The headless session end to end, with a scripted model instead of a network endpoint:
//! one chat turn, a tool round-trip, a parked approval, persisted settings, memory written by the
//! agent, and skills loaded from a temporary folder.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tempfile::TempDir;
use xiaoman_api::{
    Action, ApprovalScope, AuditRecord, AuditSink, ChatRequest, ChatResponse, ChatRole,
    GuardContext, LlmClient, LlmError, ModelConfig, ModelRole, Sentinel, ToolCall,
};
use xiaoman_app::chat_item::ChatItem;
use xiaoman_app::session::{
    LlmFactory, SendOutcome, Session, SessionConfig, SessionEvent, SessionSnapshot,
};
use xiaoman_app::settings::{AppSettings, ModelSettings};
use xiaoman_sentinel::PolicySentinel;

const KEY: &str = "ark-0123456789abcdef";

// ---------------------------------------------------------------------------------------------
// Scripted model
// ---------------------------------------------------------------------------------------------

struct ScriptedLlm {
    config: ModelConfig,
    responses: Mutex<VecDeque<ChatResponse>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ScriptedLlm {
    fn new(responses: Vec<ChatResponse>) -> Arc<Self> {
        Arc::new(Self {
            config: ModelConfig::new(
                ModelRole::Chat,
                "https://example.invalid/v1",
                "scripted",
                "test-key",
            ),
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    fn config(&self) -> &ModelConfig {
        &self.config
    }

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.requests.lock().unwrap().push(request);
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
        tool_calls: vec![ToolCall {
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

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

fn sample_settings() -> AppSettings {
    AppSettings {
        chat: ModelSettings {
            model: "doubao-chat".to_string(),
            api_key: KEY.to_string(),
            temperature: Some(0.3),
            ..ModelSettings::default()
        },
        operator: ModelSettings {
            base_url: "https://example.com/v1".to_string(),
            model: "doubao-vision".to_string(),
            api_key: KEY.to_string(),
            temperature: None,
        },
        max_steps: 12,
        learn_from_success: false,
    }
}

fn session(dir: &TempDir, llm: Arc<ScriptedLlm>) -> Session {
    let session = Session::new(SessionConfig::new(dir.path()).with_llm_factory(factory(llm)))
        .expect("session opens");
    session
        .save_settings(sample_settings())
        .expect("settings save");
    session
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

fn texts(items: &[ChatItem]) -> Vec<String> {
    items
        .iter()
        .map(|item| match item {
            ChatItem::User { text, .. }
            | ChatItem::Assistant { text, .. }
            | ChatItem::Progress { text, .. }
            | ChatItem::Question { text, .. }
            | ChatItem::Error { text, .. } => text.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[test]
fn one_chat_turn_streams_a_reply_and_lands_in_the_items() {
    let dir = TempDir::new().unwrap();
    let llm = ScriptedLlm::new(vec![reply("你好，我在。")]);
    let session = session(&dir, llm);
    let mut events = session.subscribe();

    assert_eq!(SendOutcome::Started, session.send("在吗"));
    assert!(
        session.wait_idle(Duration::from_secs(5)),
        "the turn finishes"
    );

    let snapshot = session.state();
    assert!(!snapshot.busy);
    let texts = texts(&snapshot.items);
    assert_eq!(vec!["在吗".to_string(), "你好，我在。".to_string()], texts);

    let mut replies = Vec::new();
    while let Some(event) = events.try_recv() {
        if let SessionEvent::Agent(xiaoman_api::AgentEvent::Reply { text, .. }) = event {
            replies.push(text);
        }
    }
    assert_eq!(vec!["你好，我在。".to_string()], replies);
}

#[test]
fn the_vocab_engine_is_available_to_the_model_and_persists() {
    let dir = TempDir::new().unwrap();
    let llm = ScriptedLlm::new(vec![reply("好，开始背单词。")]);
    let opened = session(&dir, llm.clone());

    let plan = opened
        .vocab()
        .expect("the session has a vocabulary engine")
        .today("2026-11-13")
        .expect("the daily plan");
    assert_eq!(plan.new_words.len(), 15);
    assert!(dir.path().join("vocab/state.json").exists());

    assert_eq!(SendOutcome::Started, opened.send("背单词"));
    assert!(opened.wait_idle(Duration::from_secs(5)));

    let request = &llm.requests()[0];
    assert!(
        request.tools.iter().any(|tool| tool.name == "vocab_today"),
        "the vocabulary tools are offered"
    );
    let system = request.messages[0].text();
    assert!(system.contains("## 背单词"), "{system}");
    assert!(system.contains("vocab_grade"), "{system}");

    drop(opened);
    let llm = ScriptedLlm::new(vec![reply("好")]);
    let reopened = session(&dir, llm);
    let again = reopened
        .vocab()
        .expect("the session has a vocabulary engine")
        .today("2026-11-13")
        .expect("the persisted daily plan");
    assert_eq!(again.new_words, plan.new_words);
}

#[test]
fn a_tool_call_round_trip_feeds_the_result_back_to_the_model() {
    let dir = TempDir::new().unwrap();
    let llm = ScriptedLlm::new(vec![
        tool_call("list_skills", "{}"),
        reply("看过了，没有技能。"),
    ]);
    let session = session(&dir, llm.clone());

    assert_eq!(SendOutcome::Started, session.send("有哪些技能？"));
    assert!(session.wait_idle(Duration::from_secs(5)));

    let requests = llm.requests();
    assert_eq!(2, requests.len(), "one request per model round");
    let second = &requests[1];
    let tool_message = second
        .messages
        .iter()
        .find(|message| message.role == ChatRole::Tool)
        .expect("the tool result is sent back");
    assert_eq!(Some("call-1"), tool_message.tool_call_id.as_deref());
    assert!(
        tool_message.text().contains("\"skills\""),
        "{}",
        tool_message.text()
    );
    assert!(
        second
            .messages
            .iter()
            .any(|message| message.role == ChatRole::Assistant && !message.tool_calls.is_empty()),
        "the assistant tool call stays in the history"
    );

    assert!(texts(&session.items()).contains(&"看过了，没有技能。".to_string()));
}

#[test]
fn an_approval_parks_and_answers_allowed_and_denied() {
    let dir = TempDir::new().unwrap();
    let session = session(&dir, ScriptedLlm::new(vec![]));
    let sentinel = Arc::new(PolicySentinel::new(
        session.approval_gateway(),
        Arc::new(NoAudit),
    ));
    let handle = session.handle();
    let action = Action::Tap {
        point: None,
        node_id: None,
        target: Some("确认呼叫".to_string()),
        reason: None,
    };

    let authorize = {
        let sentinel = sentinel.clone();
        let action = action.clone();
        let handle = handle.clone();
        std::thread::spawn(move || handle.block_on(sentinel.authorize(&context(), &action)))
    };
    wait_until(|| session.state().approval.is_some());
    let pending = session.state().approval.expect("the card is open");
    assert!(pending.prompt.contains("确认呼叫"), "{}", pending.prompt);
    assert!(
        pending.reason.contains("confirmation word"),
        "the reason names the rule: {}",
        pending.reason
    );
    assert!(session.approve(pending.id, ApprovalScope::Once));
    assert!(authorize.join().unwrap(), "an allowed action runs");
    assert!(session.state().approval.is_none(), "the card closes");
    assert!(
        texts(&session.items())
            .iter()
            .any(|text| text.starts_with("已允许：")),
        "{:?}",
        texts(&session.items())
    );

    let authorize = {
        let sentinel = sentinel.clone();
        let handle = handle.clone();
        std::thread::spawn(move || {
            handle.block_on(sentinel.authorize(
                &context(),
                &Action::Tap {
                    point: None,
                    node_id: None,
                    target: Some("确认呼叫".to_string()),
                    reason: None,
                },
            ))
        })
    };
    wait_until(|| session.state().approval.is_some());
    let pending = session.state().approval.expect("the card is open");
    assert!(session.deny(pending.id));
    assert!(!authorize.join().unwrap(), "a denied action is blocked");
    assert!(
        texts(&session.items())
            .iter()
            .any(|text| text.starts_with("已拒绝：")),
        "{:?}",
        texts(&session.items())
    );
}

#[test]
fn a_stale_approval_id_cannot_answer_the_current_card() {
    let dir = TempDir::new().unwrap();
    let session = session(&dir, ScriptedLlm::new(vec![]));
    let gateway = session.approval_gateway();
    let handle = session.handle();
    let request = |prompt: &str| xiaoman_api::ApprovalRequest {
        task_id: "task-1".to_string(),
        action: Action::Tap {
            point: None,
            node_id: None,
            target: Some("确认支付".to_string()),
            reason: None,
        },
        prompt: prompt.to_string(),
        reason: "confirmation word".to_string(),
    };

    let first = {
        let gateway = gateway.clone();
        let handle = handle.clone();
        let request = request("第一次确认");
        std::thread::spawn(move || handle.block_on(gateway.request(request)))
    };
    wait_until(|| session.state().approval.is_some());
    let first_id = session.state().approval.expect("the first card").id;

    let second = {
        let gateway = gateway.clone();
        let handle = handle.clone();
        let request = request("第二次确认");
        std::thread::spawn(move || handle.block_on(gateway.request(request)))
    };
    wait_until(|| {
        session
            .state()
            .approval
            .as_ref()
            .is_some_and(|card| card.id != first_id)
    });
    let second_id = session.state().approval.expect("the replacement card").id;

    // A card or notification that outlived its approval must not answer the newer one.
    assert!(
        !session.approve(first_id, ApprovalScope::Once),
        "a stale id is refused"
    );
    assert!(!session.deny(first_id), "a stale deny is refused too");
    assert_eq!(
        Some(second_id),
        session.state().approval.map(|card| card.id),
        "the current card is still open"
    );
    assert!(
        first.join().unwrap().is_err(),
        "replacing the card cancels the first request"
    );

    assert!(session.approve(second_id, ApprovalScope::Once));
    assert!(
        second
            .join()
            .unwrap()
            .expect("the current card answers")
            .approved
    );
    assert!(
        session.state().approval.is_none(),
        "answering closes the card"
    );
}

#[test]
fn settings_saved_by_one_session_load_in_the_next() {
    let dir = TempDir::new().unwrap();
    {
        let session = session(&dir, ScriptedLlm::new(vec![]));
        assert!(session.configured());
        assert_eq!(sample_settings(), session.settings());
    }

    let llm = ScriptedLlm::new(vec![reply("记住了。")]);
    let second = Session::new(SessionConfig::new(dir.path()).with_llm_factory(factory(llm)))
        .expect("session reopens");
    assert_eq!(sample_settings(), second.settings());
    assert!(second.configured());

    let plain = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
    assert!(plain.contains("doubao-chat"));
    assert!(!plain.contains(KEY), "the API keys stay in the secret file");
    let secret = std::fs::read_to_string(dir.path().join("settings-secrets.json")).unwrap();
    assert!(secret.contains(KEY));

    assert_eq!(SendOutcome::Started, second.send("记住我的偏好"));
    assert!(second.wait_idle(Duration::from_secs(5)));
    assert!(texts(&second.items()).contains(&"记住了。".to_string()));
}

#[test]
fn memory_written_by_the_agent_is_listed() {
    let dir = TempDir::new().unwrap();
    let llm = ScriptedLlm::new(vec![
        tool_call(
            "memory_write",
            r#"{"title":"家","observations":[{"category":"address","content":"北京市海淀区某某路 1 号"}]}"#,
        ),
        reply("记住了。"),
    ]);
    let session = session(&dir, llm);
    assert!(session.memory_list(None).unwrap().is_empty());

    assert_eq!(
        SendOutcome::Started,
        session.send("我家在北京市海淀区某某路 1 号")
    );
    assert!(session.wait_idle(Duration::from_secs(5)));

    assert!(
        texts(&session.items())
            .iter()
            .any(|text| text.contains("已更新记忆")),
        "{:?}",
        texts(&session.items())
    );
    let notes = session.memory_list(None).unwrap();
    assert_eq!(1, notes.len());
    assert_eq!("家", notes[0].title);
    assert!(
        notes[0]
            .observations
            .iter()
            .any(|item| item.content.contains("某某路"))
    );

    let hits = session.memory_search("海淀", 5).unwrap();
    assert!(!hits.is_empty(), "the note is searchable");
    assert!(dir.path().join("memory").is_dir());
}

#[test]
fn skills_are_listed_from_the_data_folder() {
    let dir = TempDir::new().unwrap();
    let skill_dir = dir.path().join("skills").join("demo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: 演示技能\nchannel: chat\n---\n# 演示\n",
    )
    .unwrap();

    let session = Session::new(SessionConfig::new(dir.path())).expect("session opens");
    let names: Vec<String> = session
        .skills_list()
        .into_iter()
        .map(|skill| skill.name)
        .collect();
    assert!(names.contains(&"demo".to_string()), "{names:?}");

    let result = session.skills_reload();
    assert!(result.skills.iter().any(|skill| skill.name == "demo"));
    assert!(result.problems.is_empty(), "{:?}", result.problems);
}

#[test]
fn a_busy_session_refuses_a_second_turn() {
    let dir = TempDir::new().unwrap();
    let (release, gate) = tokio::sync::oneshot::channel();
    let llm = Arc::new(GatedLlm {
        config: ModelConfig::new(
            ModelRole::Chat,
            "https://example.invalid/v1",
            "gated",
            "test-key",
        ),
        gate: tokio::sync::Mutex::new(Some(gate)),
    });
    let session = Session::new(SessionConfig::new(dir.path()).with_llm_factory(Arc::new({
        let llm = llm.clone();
        move |_config: &ModelConfig| llm.clone() as Arc<dyn LlmClient>
    })))
    .expect("session opens");
    session.save_settings(sample_settings()).unwrap();

    assert_eq!(SendOutcome::Started, session.send("第一句"));
    assert_eq!(SendOutcome::Busy, session.send("第二句"));
    assert_eq!(SendOutcome::Empty, session.send("   "));

    let snapshot = session.state();
    assert!(snapshot.busy);
    assert_eq!(1, snapshot.items.len(), "the busy input is not added");

    release.send(()).unwrap();
    assert!(session.wait_idle(Duration::from_secs(5)));
    let texts = texts(&session.items());
    assert_eq!(vec!["第一句".to_string(), "好的".to_string()], texts);
}

// ---------------------------------------------------------------------------------------------
// Helpers for the approval test
// ---------------------------------------------------------------------------------------------

fn context() -> GuardContext {
    GuardContext {
        task_id: "task-1".to_string(),
        goal: "打车回家".to_string(),
        skill: None,
        screen: None,
        step_index: 0,
    }
}

struct NoAudit;

impl AuditSink for NoAudit {
    fn record(&self, _record: AuditRecord) {}
}

/// Parks every request until the test releases it.
struct GatedLlm {
    config: ModelConfig,
    gate: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

#[async_trait]
impl LlmClient for GatedLlm {
    fn config(&self) -> &ModelConfig {
        &self.config
    }

    async fn complete(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let receiver = self.gate.lock().await.take();
        if let Some(receiver) = receiver {
            let _ = receiver.await;
        }
        Ok(reply("好的"))
    }
}

// Compile-time proof that the snapshot keeps its public shape for the screens.
#[allow(dead_code)]
fn snapshot_shape(snapshot: &SessionSnapshot) -> (usize, bool, bool) {
    (snapshot.items.len(), snapshot.busy, snapshot.configured)
}

// ---------------------------------------------------------------------------------------------
// The real client, against a local OpenAI-compatible endpoint
// ---------------------------------------------------------------------------------------------

/// The same turn as the first test, but with the production `OpenAiCompatibleClient` talking HTTP
/// to a mock `/chat/completions`: settings -> agent -> request -> reply -> transcript.
#[tokio::test]
async fn a_real_http_turn_round_trips_through_the_openai_client() {
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": "你好，我是小满。"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 5},
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let session = Session::new(SessionConfig::new(dir.path())).expect("session opens");
    let mut settings = sample_settings();
    settings.chat.base_url = server.uri();
    settings.operator.base_url = server.uri();
    session.save_settings(settings).expect("settings save");

    assert_eq!(SendOutcome::Started, session.send("在吗"));
    for _ in 0..600 {
        if !session.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!session.is_busy(), "the HTTP turn finishes");
    assert!(texts(&session.items()).contains(&"你好，我是小满。".to_string()));

    let requests = server
        .received_requests()
        .await
        .expect("the mock records the request");
    assert_eq!(1, requests.len());
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("the request is JSON");
    assert_eq!("doubao-chat", body["model"]);
    assert!(
        body["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|message| message["content"] == "在吗")),
        "the user message reached the endpoint"
    );
}
