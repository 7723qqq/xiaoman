//! The sentinel fixtures are shared with the Kotlin `GoldenTest`: both build the same context and
//! action, run `guard`, and compare the decision kind, prompt and reason.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use xiaoman_api::{
    Action, ApprovalAnswer, ApprovalError, ApprovalGateway, ApprovalRequest, AuditRecord,
    AuditSink, Bounds, Decision, GuardContext, ScreenState, Sentinel, Skill, SkillChannel, UiNode,
};
use xiaoman_sentinel::PolicySentinel;

struct Deny;

#[async_trait]
impl ApprovalGateway for Deny {
    async fn request(&self, _request: ApprovalRequest) -> Result<ApprovalAnswer, ApprovalError> {
        Ok(ApprovalAnswer::once(false))
    }
}

struct Quiet;

impl AuditSink for Quiet {
    fn record(&self, _record: AuditRecord) {}
}

fn skill(value: &Value) -> Skill {
    let strings = |key: &str| -> Vec<String> {
        value[key]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    let deeplinks = value["deeplinks"]
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    Skill {
        name: value["name"].as_str().unwrap_or_default().to_string(),
        description: value["description"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        apps: strings("apps"),
        channel: SkillChannel::Gui,
        confirm_before: strings("confirmBefore"),
        memory_refs: Vec::new(),
        deeplinks,
        instructions: String::new(),
        flow: None,
        source: "fixture".to_string(),
    }
}

fn node(value: &Value) -> UiNode {
    let bounds = &value["bounds"];
    UiNode {
        id: value["id"].as_i64().unwrap_or_default() as i32,
        text: value["text"].as_str().map(str::to_string),
        description: value["description"].as_str().map(str::to_string),
        resource_id: None,
        class_name: None,
        bounds: Bounds {
            left: bounds["left"].as_i64().unwrap_or_default() as i32,
            top: bounds["top"].as_i64().unwrap_or_default() as i32,
            right: bounds["right"].as_i64().unwrap_or_default() as i32,
            bottom: bounds["bottom"].as_i64().unwrap_or_default() as i32,
        },
        clickable: value["clickable"].as_bool().unwrap_or(false),
        editable: value["editable"].as_bool().unwrap_or(false),
        scrollable: false,
        focused: value["focused"].as_bool().unwrap_or(false),
        password: value["password"].as_bool().unwrap_or(false),
    }
}

fn screen(value: &Value) -> ScreenState {
    let mut screen = ScreenState::new(
        None,
        1080,
        2400,
        value["packageName"].as_str().map(str::to_string),
    );
    screen.activity_name = value["activityName"].as_str().map(str::to_string);
    screen.secure = value["secure"].as_bool().unwrap_or(false);
    screen.nodes = value["nodes"]
        .as_array()
        .map(|nodes| nodes.iter().map(node).collect())
        .unwrap_or_default();
    screen
}

#[test]
fn every_case_matches_the_shared_decision() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sentinel");
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .expect("fixtures/sentinel")
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    let mut cases = 0;
    let mut actions = 0;
    for path in paths {
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .to_string();
        let Some(case) = name.strip_suffix(".json") else {
            continue;
        };
        let root: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("case")).expect("JSON");
        let skill = root["skill"].as_object().map(|_| skill(&root["skill"]));
        let screen = root["screen"].as_object().map(|_| screen(&root["screen"]));
        let sentinel = PolicySentinel::new(Arc::new(Deny), Arc::new(Quiet));
        for (index, entry) in root["actions"]
            .as_array()
            .expect("actions")
            .iter()
            .enumerate()
        {
            let action: Action = serde_json::from_value(entry["action"].clone()).expect("action");
            let context = GuardContext {
                task_id: "fixture".to_string(),
                goal: "fixture".to_string(),
                skill: skill.clone().map(Arc::new),
                screen: screen.clone().map(Arc::new),
                step_index: 0,
            };
            let decision = pollster::block_on(sentinel.guard(&context, &action));
            let expected = &entry["decision"];
            let label = format!("{case}[{index}]");
            match expected["kind"].as_str().expect("kind") {
                "allow" => match decision {
                    Decision::Allow { reason } => assert_eq!(
                        expected["reason"].as_str().unwrap_or("allowed"),
                        reason,
                        "{label} allow reason"
                    ),
                    other => panic!("{label}: expected allow, got {other:?}"),
                },
                "deny" => match decision {
                    Decision::Deny { reason } => {
                        assert_eq!(
                            expected["reason"].as_str().expect("deny reason"),
                            reason,
                            "{label} deny reason"
                        )
                    }
                    other => panic!("{label}: expected deny, got {other:?}"),
                },
                "ask" => match decision {
                    Decision::Ask { prompt, reason } => {
                        assert_eq!(
                            expected["prompt"].as_str().expect("prompt"),
                            prompt,
                            "{label} prompt"
                        );
                        assert_eq!(
                            expected["reason"].as_str().expect("ask reason"),
                            reason,
                            "{label} reason"
                        );
                    }
                    other => panic!("{label}: expected ask, got {other:?}"),
                },
                other => panic!("{label}: unknown decision kind {other}"),
            }
            actions += 1;
        }
        cases += 1;
    }
    assert!(
        cases >= 15,
        "expected the shared sentinel fixtures, found {cases}"
    );
    assert!(actions >= 15, "expected actions, found {actions}");
}
