//! The chat turn parks a skill draft and only the card's answer writes it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tempfile::TempDir;
use xiaoman_api::{
    ChatRequest, ChatResponse, LlmClient, LlmError, ModelConfig, ModelRole, SkillAction, ToolCall,
};
use xiaoman_app::session::{LlmFactory, SendOutcome, Session, SessionConfig};
use xiaoman_app::settings::{AppSettings, ModelSettings};

const KEY: &str = "ark-0123456789abcdef";

struct ScriptedLlm {
    config: ModelConfig,
    responses: Mutex<VecDeque<ChatResponse>>,
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

fn tool_call(name: &str, arguments: serde_json::Value) -> ChatResponse {
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

fn settings() -> AppSettings {
    AppSettings {
        chat: ModelSettings {
            model: "doubao-chat".to_string(),
            api_key: KEY.to_string(),
            ..ModelSettings::default()
        },
        operator: ModelSettings {
            base_url: "https://example.com/v1".to_string(),
            model: "doubao-vision".to_string(),
            api_key: KEY.to_string(),
            ..ModelSettings::default()
        },
        ..AppSettings::default()
    }
}

fn session(dir: &TempDir, llm: Arc<ScriptedLlm>) -> Session {
    let session = Session::new(SessionConfig::new(dir.path()).with_llm_factory(factory(llm)))
        .expect("session opens");
    session.save_settings(settings()).expect("settings save");
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

fn create_call() -> serde_json::Value {
    json!({
        "action": "create",
        "name": "daily-review",
        "description": "每天复盘",
        "channel": "chat",
        "instructions": "依次问今天做成的三件事、明天最重要的一件事，然后写入记忆。",
        "memory": ["preferences/复盘"],
    })
}

fn start_draft(dir: &TempDir) -> (Session, i64) {
    let llm = ScriptedLlm::new(vec![
        tool_call("skill_edit", create_call()),
        reply("技能草稿已准备好。"),
    ]);
    let session = session(dir, llm);
    assert_eq!(SendOutcome::Started, session.send("帮我加一个复盘技能"));
    wait_until(|| session.state().skill_draft.is_some());
    let draft = session.state().skill_draft.expect("draft");
    (session, draft.id)
}

#[test]
fn snapshot_carries_the_draft_before_anything_is_written() {
    let dir = TempDir::new().unwrap();
    let (session, _) = start_draft(&dir);

    let draft = session.state().skill_draft.expect("draft");
    assert_eq!(draft.action, SkillAction::Create);
    assert_eq!(draft.name, "daily-review");
    assert!(draft.content.contains("每天复盘"));
    assert!(!draft.shadows_bundled);
    assert!(!dir.path().join("skills/daily-review/SKILL.md").exists());
    session.answer_skill_draft(draft.id, false);
    assert!(session.wait_idle(Duration::from_secs(5)));
}

#[test]
fn save_by_id_writes_and_lists_the_skill() {
    let dir = TempDir::new().unwrap();
    let (session, id) = start_draft(&dir);

    assert!(session.answer_skill_draft(id, true));
    assert!(session.wait_idle(Duration::from_secs(5)));

    assert!(dir.path().join("skills/daily-review/SKILL.md").is_file());
    assert!(
        session
            .skills_list()
            .iter()
            .any(|skill| skill.name == "daily-review")
    );
    assert!(session.state().skill_draft.is_none());
}

#[test]
fn cancel_by_id_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let (session, id) = start_draft(&dir);

    assert!(session.answer_skill_draft(id, false));
    assert!(session.wait_idle(Duration::from_secs(5)));

    assert!(!dir.path().join("skills/daily-review").exists());
    assert!(session.state().skill_draft.is_none());
}

#[test]
fn stale_id_does_not_answer_the_current_draft() {
    let dir = TempDir::new().unwrap();
    let (session, id) = start_draft(&dir);

    assert!(!session.answer_skill_draft(id + 1, true));
    assert!(session.state().skill_draft.is_some());
    assert!(!dir.path().join("skills/daily-review").exists());

    assert!(session.answer_skill_draft(id, false));
    assert!(session.wait_idle(Duration::from_secs(5)));
}
