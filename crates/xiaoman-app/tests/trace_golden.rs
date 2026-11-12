//! The trace fixtures are shared with the Kotlin `TraceGoldenTest`: every event renders to the same
//! JSON line, key order and nulls included.

use std::fs;
use std::path::Path;

use serde_json::Value;
use xiaoman_api::{Action, ActionResult, AgentEvent, Decision};
use xiaoman_app::trace;

const TIME: i64 = 1_761_830_000_000;

fn decision(value: &Value) -> Decision {
    match value["kind"].as_str().unwrap() {
        "allow" => Decision::Allow {
            reason: value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("allowed")
                .to_string(),
        },
        "ask" => Decision::Ask {
            prompt: value["prompt"].as_str().unwrap_or_default().to_string(),
            reason: value["reason"].as_str().unwrap_or_default().to_string(),
        },
        "deny" => Decision::Deny {
            reason: value["reason"].as_str().unwrap_or_default().to_string(),
        },
        other => panic!("{other}"),
    }
}

fn event(value: &Value) -> AgentEvent {
    let task_id = "r1".to_string();
    let action = || serde_json::from_value::<Action>(value["action"].clone()).unwrap();
    match value["kind"].as_str().unwrap() {
        "Reply" => AgentEvent::Reply {
            task_id,
            text: value["text"].as_str().unwrap().to_string(),
        },
        "SkillChosen" => AgentEvent::SkillChosen {
            task_id,
            skill_name: value["text"].as_str().unwrap().to_string(),
        },
        "PhoneTaskStarted" => AgentEvent::PhoneTaskStarted {
            task_id,
            goal: value["text"].as_str().unwrap().to_string(),
            skill_name: None,
        },
        "StepPlanned" => AgentEvent::StepPlanned {
            task_id,
            step_index: value["step"].as_i64().unwrap() as i32,
            action: action(),
        },
        "StepExecuted" => AgentEvent::StepExecuted {
            task_id,
            step_index: value["step"].as_i64().unwrap() as i32,
            action: action(),
            result: ActionResult {
                ok: value["ok"].as_bool().unwrap(),
                message: value["text"].as_str().map(str::to_string),
            },
        },
        "StepBlocked" => AgentEvent::StepBlocked {
            task_id,
            step_index: value["step"].as_i64().unwrap() as i32,
            action: action(),
            decision: decision(&value["decision"]),
        },
        "NeedsUser" => AgentEvent::NeedsUser {
            task_id,
            question: value["text"].as_str().unwrap().to_string(),
            takeover: value["takeover"].as_bool().unwrap_or(false),
        },
        "MemoryUpdated" => AgentEvent::MemoryUpdated {
            task_id,
            permalink: value["text"].as_str().unwrap().to_string(),
        },
        "ToolCalled" => AgentEvent::ToolCalled {
            task_id,
            tool: value["tool"].as_str().unwrap().to_string(),
            ok: value["ok"].as_bool().unwrap(),
            summary: value["summary"].as_str().unwrap().to_string(),
        },
        "PhoneTaskFinished" => AgentEvent::PhoneTaskFinished {
            task_id,
            success: value["ok"].as_bool().unwrap(),
            summary: value["text"].as_str().unwrap().to_string(),
            steps: value["steps"].as_i64().unwrap() as i32,
        },
        "Failed" => AgentEvent::Failed {
            task_id,
            message: value["text"].as_str().unwrap().to_string(),
        },
        other => panic!("{other}"),
    }
}

#[test]
fn every_event_renders_the_shared_line() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/trace");
    let events: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("events.json")).unwrap()).unwrap();
    let lines = fs::read_to_string(dir.join("lines.jsonl")).unwrap();
    let lines: Vec<&str> = lines
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let events = events.as_array().unwrap();
    assert_eq!(events.len(), lines.len(), "one line per event");
    assert!(events.len() >= 11, "expected the shared trace fixtures");
    for (index, value) in events.iter().enumerate() {
        let decision = value["decision"].as_str();
        let line = serde_json::to_string(&trace::line(&event(value), TIME, decision)).unwrap();
        assert_eq!(lines[index], line, "event {index}");
    }
}
