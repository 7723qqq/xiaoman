//! The "复盘" example end to end: propose, confirm, save, list — and never touch skills/examples.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tempfile::TempDir;
use xiaoman_api::{
    ChatRequest, ChatResponse, LlmClient, LlmError, ModelConfig, ModelRole, SkillChannel, ToolCall,
};
use xiaoman_app::session::{LlmFactory, SendOutcome, Session, SessionConfig};
use xiaoman_app::settings::{AppSettings, ModelSettings};
use xiaoman_skills::SkillParser;

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
            .unwrap_or_else(|| ChatResponse {
                text: "技能已保存，之后说“复盘”就行。".to_string(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_string()),
                usage: None,
            }))
    }
}

fn tool_call(arguments: serde_json::Value) -> ChatResponse {
    ChatResponse {
        text: String::new(),
        tool_calls: vec![ToolCall {
            id: "call-1".to_string(),
            name: "skill_edit".to_string(),
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

fn walk(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// FNV-1a over every relative path and its bytes, so any edit under the tree changes it.
fn tree_hash(root: &Path) -> u64 {
    let mut files = Vec::new();
    walk(root, &mut files);
    files.sort();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for path in files {
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let bytes = fs::read(&path).unwrap_or_default();
        for byte in relative
            .to_string_lossy()
            .bytes()
            .chain(bytes.iter().copied())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[test]
fn the_review_skill_is_saved_only_after_the_card_is_answered() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/examples");
    let before = tree_hash(&examples);

    let dir = TempDir::new().unwrap();
    let llm = ScriptedLlm::new(vec![tool_call(json!({
        "action": "create",
        "name": "daily-review",
        "description": "每天晚上说“复盘”：问今天做成的三件事和明天最重要的一件事，并写进记忆",
        "channel": "chat",
        "instructions": "1. 依次问今天做成了哪三件事。\n2. 问明天最重要的一件事是什么。\n3. 用 memory_write 把回答记进记忆，不要猜测。",
        "memory": ["preferences/复盘"],
    }))]);
    let session = Session::new(SessionConfig::new(dir.path()).with_llm_factory(factory(llm)))
        .expect("session opens");
    session.save_settings(settings()).expect("settings save");

    assert_eq!(
        SendOutcome::Started,
        session.send("帮我加一个技能，每天晚上我说“复盘”，就问我今天做成了哪三件事、明天最重要的一件是什么，答完记进记忆")
    );
    wait_until(|| session.state().skill_draft.is_some());
    let draft = session.state().skill_draft.expect("draft");
    assert_eq!(draft.name, "daily-review");
    assert!(draft.content.contains("三件事"));
    assert!(!dir.path().join("skills/daily-review/SKILL.md").exists());
    assert_eq!(
        tree_hash(&examples),
        before,
        "the examples tree was touched"
    );

    assert!(session.answer_skill_draft(draft.id, true));
    assert!(session.wait_idle(Duration::from_secs(5)));

    let saved = dir.path().join("skills/daily-review/SKILL.md");
    assert!(saved.is_file());
    let (skill, problems) = SkillParser::parse_folder(
        "daily-review",
        &fs::read_to_string(&saved).unwrap(),
        None,
        "test",
    );
    assert!(problems.is_empty(), "{problems:?}");
    let skill = skill.expect("skill");
    assert_eq!(skill.channel, SkillChannel::Chat);
    assert!(skill.instructions.contains("memory_write"));
    assert!(
        session
            .skills_list()
            .iter()
            .any(|skill| skill.name == "daily-review")
    );
    assert_eq!(
        tree_hash(&examples),
        before,
        "the examples tree was touched"
    );
}
