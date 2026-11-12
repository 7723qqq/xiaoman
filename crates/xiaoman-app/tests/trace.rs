//! Ports of `app/src/test/kotlin/dev/xiaoman/app/trace/TraceWriterTest.kt`.
//!
//! The first test also pins the raw `trace.jsonl` line, so the wire format stays byte-compatible
//! with the Kotlin writer: the eight keys in order, absent values as `null`, compact JSON.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tempfile::TempDir;
use xiaoman_api::{
    Action, ActionResult, AgentEvent, AgentListener, AuditRecord, AuditSink, Decision, Point,
};
use xiaoman_app::trace::{self, AUDIT_FILE, TRACE_FILE, TraceWriter};

const KEYS: [&str; 8] = [
    "time", "run", "kind", "step", "action", "text", "ok", "decision",
];

fn lines(path: &std::path::Path) -> Vec<Value> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    text.lines()
        .map(|line| serde_json::from_str(line).expect("every line is JSON"))
        .collect()
}

fn tap() -> Action {
    Action::Tap {
        point: None,
        node_id: Some(12),
        target: Some("目的地输入框".to_string()),
        reason: Some("先输入目的地".to_string()),
    }
}

#[test]
fn every_line_has_exactly_the_documented_keys_in_order() {
    let tmp = TempDir::new().unwrap();
    let logged = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&logged);
    let writer = TraceWriter::new(tmp.path())
        .with_clock(|| 1_761_830_000_000)
        .with_log(move |line| sink.lock().unwrap().push(line.to_string()));

    writer.on_event(&AgentEvent::PhoneTaskStarted {
        task_id: "r1".to_string(),
        goal: "打车回家".to_string(),
        skill_name: None,
    });
    writer.on_event(&AgentEvent::StepPlanned {
        task_id: "r1".to_string(),
        step_index: 0,
        action: tap(),
    });
    writer.record(AuditRecord {
        time_millis: 1_761_830_000_001,
        task_id: "r1".to_string(),
        step_index: 0,
        action: tap(),
        decision: Decision::Allow {
            reason: "no rule matched".to_string(),
        },
        approved: None,
    });
    writer.on_event(&AgentEvent::StepExecuted {
        task_id: "r1".to_string(),
        step_index: 0,
        action: tap(),
        result: ActionResult {
            ok: true,
            message: None,
        },
    });
    writer.on_event(&AgentEvent::StepBlocked {
        task_id: "r1".to_string(),
        step_index: 1,
        action: Action::Tap {
            point: Some(Point::new(500, 900).unwrap()),
            node_id: None,
            target: Some("确认呼叫".to_string()),
            reason: None,
        },
        decision: Decision::Ask {
            prompt: "确认呼叫？".to_string(),
            reason: "confirmation word '确认呼叫'".to_string(),
        },
    });
    writer.on_event(&AgentEvent::PhoneTaskFinished {
        task_id: "r1".to_string(),
        success: true,
        summary: "已停在确认呼叫".to_string(),
        steps: 1,
    });

    let trace_path = tmp.path().join("r1").join(TRACE_FILE);
    let trace = lines(&trace_path);
    assert_eq!(trace.len(), 5);
    for line in &trace {
        let keys: Vec<&str> = line
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, KEYS.to_vec());
    }

    let first = std::fs::read_to_string(&trace_path).unwrap();
    assert_eq!(
        first.lines().next().unwrap(),
        r#"{"time":1761830000000,"run":"r1","kind":"PhoneTaskStarted","step":null,"action":null,"text":"打车回家","ok":null,"decision":null}"#
    );

    let started = &trace[0];
    assert_eq!(started["time"], json!(1_761_830_000_000i64));
    assert_eq!(started["run"], json!("r1"));
    assert_eq!(started["kind"], json!("PhoneTaskStarted"));
    assert_eq!(started["text"], json!("打车回家"));
    assert_eq!(started["step"], Value::Null);
    assert_eq!(started["action"], Value::Null);
    assert_eq!(started["ok"], Value::Null);
    assert_eq!(started["decision"], Value::Null);

    assert_eq!(
        trace[1]["action"],
        json!({"type": "tap", "nodeId": 12, "target": "目的地输入框", "reason": "先输入目的地"})
    );
    assert_eq!(trace[1]["step"], json!(0));

    let executed = &trace[2];
    assert_eq!(executed["kind"], json!("StepExecuted"));
    assert_eq!(executed["ok"], json!(true));
    assert_eq!(executed["decision"], json!("allow: no rule matched"));

    let blocked = &trace[3];
    assert_eq!(
        blocked["decision"],
        json!("ask: confirmation word '确认呼叫'")
    );
    assert_eq!(blocked["ok"], json!(false));
    assert_eq!(blocked["action"]["point"], json!({"x": 500, "y": 900}));

    let finished = &trace[4];
    assert_eq!(finished["step"], json!(1));
    assert_eq!(finished["ok"], json!(true));
    assert_eq!(finished["text"], json!("已停在确认呼叫"));

    // logcat: "<run_id> <EventKind> <json>" with the same object as the trace line.
    let logged = logged.lock().unwrap();
    assert_eq!(logged.len(), 5);
    let mut parts = logged[2].splitn(3, ' ');
    assert_eq!(parts.next(), Some("r1"));
    assert_eq!(parts.next(), Some("StepExecuted"));
    let json_text = parts.next().expect("the json payload");
    assert_eq!(serde_json::from_str::<Value>(json_text).unwrap(), *executed);

    let audit = lines(&tmp.path().join("r1").join(AUDIT_FILE));
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0]["decision"], json!("allow: no rule matched"));
    assert_eq!(audit[0]["step"], json!(0));
    assert_eq!(audit[0]["approved"], Value::Null);
}

#[test]
fn launch_actions_serialize_package_name_and_failures_carry_the_message() {
    let tmp = TempDir::new().unwrap();
    let writer = TraceWriter::new(tmp.path()).with_clock(|| 5);
    writer.on_event(&AgentEvent::StepPlanned {
        task_id: "r2".to_string(),
        step_index: 0,
        action: Action::LaunchApp {
            package_name: "com.android.settings".to_string(),
            target: None,
            reason: None,
        },
    });
    writer.on_event(&AgentEvent::Failed {
        task_id: "r2".to_string(),
        message: "accessibility service not connected".to_string(),
    });

    let trace = lines(&tmp.path().join("r2").join(TRACE_FILE));
    assert_eq!(
        trace[0]["action"],
        json!({"type": "launch", "packageName": "com.android.settings"})
    );
    assert_eq!(trace[1]["kind"], json!("Failed"));
    assert_eq!(
        trace[1]["text"],
        json!("accessibility service not connected")
    );
    assert_eq!(trace[1]["ok"], Value::Null);
}

#[test]
fn api_keys_are_masked_in_traces_and_logs() {
    let tmp = TempDir::new().unwrap();
    let logged = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&logged);
    let writer = TraceWriter::new(tmp.path())
        .with_secrets(|| vec!["sk-secret-123".to_string()])
        .with_log(move |line| sink.lock().unwrap().push(line.to_string()));
    writer.on_event(&AgentEvent::Reply {
        task_id: "r3".to_string(),
        text: "your key is sk-secret-123".to_string(),
    });

    let text = std::fs::read_to_string(tmp.path().join("r3").join(TRACE_FILE)).unwrap();
    assert!(!text.contains("sk-secret-123"), "{text}");
    assert!(text.contains("***"), "{text}");
    let logged = logged.lock().unwrap();
    assert!(logged.iter().all(|line| !line.contains("sk-secret-123")));
}

#[test]
fn short_secrets_are_not_masked() {
    let tmp = TempDir::new().unwrap();
    let writer = TraceWriter::new(tmp.path()).with_secrets(|| vec!["ab".to_string()]);
    writer.on_event(&AgentEvent::Reply {
        task_id: "r5".to_string(),
        text: "ab".to_string(),
    });
    let text = std::fs::read_to_string(tmp.path().join("r5").join(TRACE_FILE)).unwrap();
    assert!(text.contains("\"text\":\"ab\""), "{text}");
}

#[test]
fn screenshots_are_saved_per_run_with_the_next_step_index() {
    let tmp = TempDir::new().unwrap();
    let writer = TraceWriter::new(tmp.path());
    writer.save_screenshot(&[1]); // no running task: ignored
    writer.on_event(&AgentEvent::PhoneTaskStarted {
        task_id: "r4".to_string(),
        goal: "goal".to_string(),
        skill_name: None,
    });
    writer.save_screenshot(&[1]);
    writer.on_event(&AgentEvent::StepExecuted {
        task_id: "r4".to_string(),
        step_index: 0,
        action: Action::Back {
            target: None,
            reason: None,
        },
        result: ActionResult {
            ok: true,
            message: None,
        },
    });
    writer.save_screenshot(&[2]);
    writer.on_event(&AgentEvent::PhoneTaskFinished {
        task_id: "r4".to_string(),
        success: true,
        summary: "ok".to_string(),
        steps: 1,
    });
    writer.save_screenshot(&[3]);

    let dir = tmp.path().join("r4");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["step-0.png", "step-1.png", TRACE_FILE]);
    assert_eq!(std::fs::read(dir.join("step-1.png")).unwrap(), vec![2]);
}

#[test]
fn run_ids_cannot_escape_the_traces_folder() {
    assert_eq!(TraceWriter::safe_name("/../x"), "_.._x");
    assert_eq!(TraceWriter::safe_name("r1"), "r1");
    assert_eq!(TraceWriter::safe_name("a b"), "a_b");
    assert_eq!(TraceWriter::safe_name("..."), "_");
}

#[test]
fn a_decision_is_joined_only_onto_its_own_step() {
    let tmp = TempDir::new().unwrap();
    let writer = TraceWriter::new(tmp.path());
    writer.on_event(&AgentEvent::PhoneTaskStarted {
        task_id: "r6".to_string(),
        goal: "goal".to_string(),
        skill_name: None,
    });
    writer.record(AuditRecord {
        time_millis: 10,
        task_id: "r6".to_string(),
        step_index: 3,
        action: Action::Back {
            target: None,
            reason: None,
        },
        decision: Decision::Deny {
            reason: "blocked".to_string(),
        },
        approved: None,
    });
    writer.on_event(&AgentEvent::StepExecuted {
        task_id: "r6".to_string(),
        step_index: 0,
        action: Action::Back {
            target: None,
            reason: None,
        },
        result: ActionResult {
            ok: false,
            message: Some("denied".to_string()),
        },
    });
    writer.on_event(&AgentEvent::StepExecuted {
        task_id: "r6".to_string(),
        step_index: 3,
        action: Action::Back {
            target: None,
            reason: None,
        },
        result: ActionResult {
            ok: false,
            message: Some("denied".to_string()),
        },
    });

    let trace = lines(&tmp.path().join("r6").join(TRACE_FILE));
    assert_eq!(
        trace[1]["decision"],
        Value::Null,
        "a decision for another step is not joined"
    );
    assert_eq!(trace[2]["decision"], json!("deny: blocked"));
}

#[test]
fn tool_calls_carry_tool_and_summary() {
    let tmp = TempDir::new().unwrap();
    TraceWriter::new(tmp.path()).on_event(&AgentEvent::ToolCalled {
        task_id: "r7".to_string(),
        tool: "memory_search".to_string(),
        ok: true,
        summary: "找到 2 条".to_string(),
    });
    let trace = lines(&tmp.path().join("r7").join(TRACE_FILE));
    assert_eq!(trace[0]["text"], json!("memory_search: 找到 2 条"));
    assert_eq!(trace[0]["ok"], json!(true));
}

#[test]
fn a_failed_write_warns_instead_of_stopping_the_task() {
    let tmp = TempDir::new().unwrap();
    let blocked = tmp.path().join("blocked");
    std::fs::write(&blocked, "not a directory").unwrap();
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&warnings);
    let writer = TraceWriter::new(&blocked)
        .with_warn(move |line| sink.lock().unwrap().push(line.to_string()));

    // Must not panic even though `<file>/r8/trace.jsonl` cannot exist.
    writer.on_event(&AgentEvent::Reply {
        task_id: "r8".to_string(),
        text: "hi".to_string(),
    });

    let warnings = warnings.lock().unwrap();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains(TRACE_FILE), "{}", warnings[0]);
}

#[test]
fn the_writer_can_serve_as_the_agents_listener() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TraceWriter>();
}

#[test]
fn formats_match_the_documented_shape() {
    assert_eq!(trace::kind(&agent_event()), "Reply");
    assert_eq!(
        trace::decision_text(&Decision::Allow {
            reason: "ok".to_string()
        }),
        "allow: ok"
    );
    assert_eq!(
        trace::decision_text(&Decision::Deny {
            reason: "no".to_string()
        }),
        "deny: no"
    );
    assert_eq!(
        trace::decision_text(&Decision::Ask {
            prompt: "?".to_string(),
            reason: "confirm".to_string()
        }),
        "ask: confirm"
    );
    assert_eq!(
        trace::action(&Action::OpenDeeplink {
            uri: "app://x?a=1".to_string(),
            package_name: None,
            target: None,
            reason: None,
        }),
        json!({"type": "deeplink", "uri": "app://x?a=1"})
    );
}

#[test]
fn every_event_kind_and_text_matches_the_kotlin_trace() {
    let task = || "r1".to_string();
    let back = || Action::Back {
        target: None,
        reason: None,
    };
    let events: Vec<(&str, AgentEvent, Value)> = vec![
        (
            "Reply",
            AgentEvent::Reply {
                task_id: task(),
                text: "你好".to_string(),
            },
            json!("你好"),
        ),
        (
            "SkillChosen",
            AgentEvent::SkillChosen {
                task_id: task(),
                skill_name: "外卖".to_string(),
            },
            json!("外卖"),
        ),
        (
            "PhoneTaskStarted",
            AgentEvent::PhoneTaskStarted {
                task_id: task(),
                goal: "打车".to_string(),
                skill_name: None,
            },
            json!("打车"),
        ),
        (
            "StepPlanned",
            AgentEvent::StepPlanned {
                task_id: task(),
                step_index: 0,
                action: back(),
            },
            Value::Null,
        ),
        (
            "StepExecuted",
            AgentEvent::StepExecuted {
                task_id: task(),
                step_index: 0,
                action: back(),
                result: ActionResult {
                    ok: true,
                    message: Some("到了".to_string()),
                },
            },
            json!("到了"),
        ),
        (
            "StepBlocked",
            AgentEvent::StepBlocked {
                task_id: task(),
                step_index: 0,
                action: back(),
                decision: Decision::Deny {
                    reason: "no".to_string(),
                },
            },
            Value::Null,
        ),
        (
            "NeedsUser",
            AgentEvent::NeedsUser {
                task_id: task(),
                question: "验证码？".to_string(),
                takeover: false,
            },
            json!("验证码？"),
        ),
        (
            "MemoryUpdated",
            AgentEvent::MemoryUpdated {
                task_id: task(),
                permalink: "memory/x.md".to_string(),
            },
            json!("memory/x.md"),
        ),
        (
            "ToolCalled",
            AgentEvent::ToolCalled {
                task_id: task(),
                tool: "memory_search".to_string(),
                ok: true,
                summary: "2 条".to_string(),
            },
            json!("memory_search: 2 条"),
        ),
        (
            "PhoneTaskFinished",
            AgentEvent::PhoneTaskFinished {
                task_id: task(),
                success: false,
                summary: "没付".to_string(),
                steps: 2,
            },
            json!("没付"),
        ),
        (
            "Failed",
            AgentEvent::Failed {
                task_id: task(),
                message: "断网".to_string(),
            },
            json!("断网"),
        ),
    ];
    assert_eq!(events.len(), 11, "every AgentEvent variant is covered");
    for (name, event, text) in &events {
        assert_eq!(trace::kind(event), *name);
        let line = trace::line(event, 1, None);
        assert_eq!(line["kind"], json!(*name), "{name}");
        assert_eq!(&line["text"], text, "{name}");
        assert_eq!(line["run"], json!("r1"));
    }
}

fn agent_event() -> AgentEvent {
    AgentEvent::Reply {
        task_id: "r".to_string(),
        text: "hi".to_string(),
    }
}
