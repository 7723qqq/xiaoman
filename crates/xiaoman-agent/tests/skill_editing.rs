//! `skill_edit` only writes after the draft gateway says Save.

use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tempfile::TempDir;
use xiaoman_agent::{ChatTools, SkillEditing, agent_tools};
use xiaoman_api::{
    AgentEvent, MemoryError, MemoryStore, Note, Observation, ReminderScheduler, SearchHit, Skill,
    SkillAuthoring, SkillChannel, SkillDraft, SkillDraftAnswer, SkillDraftGateway, SkillFields,
    SkillLoadResult, SkillRepository, ToolCall,
};
use xiaoman_skills::{DirectorySkillAuthoring, render_skill_md};

struct NoMemory;

#[async_trait]
impl MemoryStore for NoMemory {
    async fn write(&self, _note: Note) -> Result<Note, MemoryError> {
        Err(MemoryError("unused".to_string()))
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
        Err(MemoryError("unused".to_string()))
    }
    async fn delete(&self, _identifier: &str) -> Result<bool, MemoryError> {
        Ok(false)
    }
    async fn list(&self, _folder: Option<&str>) -> Result<Vec<Note>, MemoryError> {
        Ok(Vec::new())
    }
}

struct NoReminders;

impl ReminderScheduler for NoReminders {
    fn schedule(&self, _text: &str, _at_millis: i64) {}
}

struct CountingSkills {
    reloads: Arc<AtomicUsize>,
}

#[async_trait]
impl SkillRepository for CountingSkills {
    async fn reload(&self) -> SkillLoadResult {
        self.reloads.fetch_add(1, Ordering::SeqCst);
        SkillLoadResult::default()
    }
    async fn list(&self) -> Vec<Skill> {
        Vec::new()
    }
    async fn get(&self, _name: &str) -> Option<Skill> {
        None
    }
}

struct FixedGateway {
    answer: SkillDraftAnswer,
    calls: Arc<AtomicUsize>,
    drafts: Arc<Mutex<Vec<SkillDraft>>>,
}

#[async_trait]
impl SkillDraftGateway for FixedGateway {
    async fn request(&self, draft: SkillDraft) -> Result<SkillDraftAnswer, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.drafts.lock().unwrap().push(draft);
        Ok(self.answer)
    }
}

struct Harness {
    _bundled: TempDir,
    user: TempDir,
    gateway: Arc<FixedGateway>,
    tools: ChatTools,
}

fn harness(answer: SkillDraftAnswer) -> Harness {
    let bundled = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let authoring: Arc<dyn SkillAuthoring> = Arc::new(DirectorySkillAuthoring::new(
        vec![bundled.path().to_path_buf()],
        user.path(),
    ));
    let gateway = Arc::new(FixedGateway {
        answer,
        calls: Arc::new(AtomicUsize::new(0)),
        drafts: Arc::new(Mutex::new(Vec::new())),
    });
    let reloads = Arc::new(AtomicUsize::new(0));
    let tools = ChatTools {
        memory: Arc::new(NoMemory),
        skills: Arc::new(CountingSkills {
            reloads: reloads.clone(),
        }),
        clock: Arc::new(|| 0),
        reminders: Arc::new(NoReminders),
        external: None,
        phone_tasks: true,
        skill_editing: Some(SkillEditing {
            authoring: authoring.clone(),
            drafts: gateway.clone(),
        }),
        vocab: None,
        utc_offset_minutes: None,
    };
    Harness {
        _bundled: bundled,
        user,
        gateway,
        tools,
    }
}

fn gui_skill(name: &str) -> String {
    let mut fields = SkillFields::new(name, "打车回家", SkillChannel::Gui, "先读记忆，再打车。");
    fields.apps = vec!["com.autonavi.minimap".to_string()];
    fields.confirm_before = vec!["确认呼叫".to_string()];
    fields.deeplinks = vec![(
        "route".to_string(),
        "amapuri://route/plan/?dname={destination}".to_string(),
    )];
    render_skill_md(&fields)
}

fn write_bundled(h: &Harness, name: &str) {
    let dir = h._bundled.path().join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), gui_skill(name)).unwrap();
}

async fn call(tools: &ChatTools, name: &str, arguments: Value) -> Value {
    let text = tools
        .execute(
            &ToolCall {
                id: "call-1".to_string(),
                name: name.to_string(),
                arguments_json: arguments.to_string(),
            },
            "task-1",
            &|_: AgentEvent| {},
        )
        .await;
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

fn create_args() -> Value {
    json!({
        "action": "create",
        "name": "daily-review",
        "description": "每天复盘",
        "channel": "chat",
        "instructions": "依次问今天做成的三件事、明天最重要的一件事，然后写入记忆。",
        "memory": ["preferences/复盘"],
    })
}

#[tokio::test]
async fn read_returns_content_and_bundled_flag() {
    let h = harness(SkillDraftAnswer::Cancel);
    write_bundled(&h, "taxi-home");
    let value = call(
        &h.tools,
        agent_tools::SKILL_READ,
        json!({"name": "taxi-home"}),
    )
    .await;
    assert_eq!(value["found"], true);
    assert_eq!(value["bundled"], true);
    assert!(value["content"].as_str().unwrap().contains("先读记忆"));
}

#[tokio::test]
async fn save_after_gateway_save_writes_and_reloads() {
    let h = harness(SkillDraftAnswer::Save);
    let value = call(&h.tools, agent_tools::SKILL_EDIT, create_args()).await;
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["action"], "create");
    assert!(h.user.path().join("daily-review/SKILL.md").is_file());
    assert_eq!(h.gateway.calls.load(Ordering::SeqCst), 1);
    let draft = h.gateway.drafts.lock().unwrap()[0].clone();
    assert!(draft.content.contains("每天复盘"));
    assert!(!draft.shadows_bundled);
}

#[tokio::test]
async fn cancel_writes_nothing() {
    let h = harness(SkillDraftAnswer::Cancel);
    let value = call(&h.tools, agent_tools::SKILL_EDIT, create_args()).await;
    assert_eq!(value["cancelled"], true, "{value}");
    assert!(!h.user.path().join("daily-review").exists());
    assert_eq!(h.gateway.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn secret_is_rejected_before_the_card() {
    let h = harness(SkillDraftAnswer::Save);
    let mut args = create_args();
    args["instructions"] = json!("有问题打 13812345678。");
    let value = call(&h.tools, agent_tools::SKILL_EDIT, args).await;
    assert!(
        value["error"]
            .as_str()
            .unwrap_or_default()
            .contains("手机号"),
        "{value}"
    );
    assert!(!value.to_string().contains("13812345678"));
    assert_eq!(h.gateway.calls.load(Ordering::SeqCst), 0);
    assert!(!h.user.path().join("daily-review").exists());
}

#[tokio::test]
async fn builtin_delete_is_rejected_before_the_card() {
    let h = harness(SkillDraftAnswer::Save);
    write_bundled(&h, "taxi-home");
    let value = call(
        &h.tools,
        agent_tools::SKILL_EDIT,
        json!({"action": "delete", "name": "taxi-home"}),
    )
    .await;
    assert!(
        value["error"].as_str().unwrap_or_default().contains("内置"),
        "{value}"
    );
    assert_eq!(h.gateway.calls.load(Ordering::SeqCst), 0);
    assert!(h._bundled.path().join("taxi-home/SKILL.md").is_file());
}

#[test]
fn no_editing_means_no_tool() {
    let names = |editing| -> Vec<String> {
        agent_tools::specs_for(editing, true)
            .into_iter()
            .map(|spec| spec.name)
            .collect()
    };
    assert!(names(true).contains(&agent_tools::SKILL_EDIT.to_string()));
    assert!(!names(false).contains(&agent_tools::SKILL_EDIT.to_string()));
    assert!(!names(false).contains(&agent_tools::SKILL_READ.to_string()));
    assert!(
        !agent_tools::specs_for(true, false)
            .iter()
            .any(|spec| spec.name == agent_tools::PHONE_TASK)
    );
}

#[tokio::test]
async fn update_shadows_a_bundled_skill_and_keeps_its_flow() {
    let h = harness(SkillDraftAnswer::Save);
    write_bundled(&h, "taxi-home");
    let flow = "steps:\n  - wait: 1500\n";
    fs::write(h._bundled.path().join("taxi-home/flow.yaml"), flow).unwrap();

    let mut args = create_args();
    args["action"] = json!("update");
    args["name"] = json!("taxi-home");
    args["description"] = json!("打车回家（用户版）");
    args["channel"] = json!("gui");
    args["instructions"] = json!("用户自己的打车说明。");
    args["apps"] = json!(["com.autonavi.minimap"]);
    args["deeplinks"] = json!({"route": "amapuri://route/plan/?dname={destination}"});
    let value = call(&h.tools, agent_tools::SKILL_EDIT, args).await;
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(
        fs::read_to_string(h.user.path().join("taxi-home/flow.yaml")).unwrap(),
        flow
    );
    let draft = h.gateway.drafts.lock().unwrap()[0].clone();
    assert!(draft.shadows_bundled);
}

#[tokio::test]
async fn missing_editing_gate_returns_an_error() {
    let tools = ChatTools {
        memory: Arc::new(NoMemory),
        skills: Arc::new(CountingSkills {
            reloads: Arc::new(AtomicUsize::new(0)),
        }),
        clock: Arc::new(|| 0),
        reminders: Arc::new(NoReminders),
        external: None,
        phone_tasks: true,
        skill_editing: None,
        vocab: None,
        utc_offset_minutes: None,
    };
    let value = call(&tools, agent_tools::SKILL_EDIT, create_args()).await;
    assert!(
        value["error"]
            .as_str()
            .unwrap_or_default()
            .contains("不支持编辑技能"),
        "{value}"
    );
}
