use std::path::{Path, PathBuf};

use serde_json::json;
use xiaoman_cli::logcat::{format_event, parse_line};
use xiaoman_cli::traces::{
    Zone, display_width, load_trace, render_table, resolve_trace, summarize_action, trace_footer,
    truncate,
};

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn trace_dir() -> PathBuf {
    repo("crates/xiaoman-cli/tests/fixtures/traces/r1")
}

#[test]
fn load() {
    let events = load_trace(&trace_dir().join("trace.jsonl")).expect("load trace");
    assert_eq!(events.len(), 7);
    assert_eq!(events[5]["kind"], json!("<invalid>"));
    assert_eq!(events[5]["_line"], json!(6));
}

#[test]
fn resolve() {
    let path = trace_dir().join("trace.jsonl");
    assert_eq!(
        resolve_trace(path.to_str().unwrap(), Path::new("traces")),
        Some(path.clone())
    );
    assert_eq!(
        resolve_trace(trace_dir().to_str().unwrap(), Path::new("traces")),
        Some(path.clone())
    );
    let root = trace_dir().parent().unwrap().to_path_buf();
    assert_eq!(resolve_trace("r1", &root), Some(path.clone()));
    assert_eq!(resolve_trace("nope", &root), None);
}

#[test]
fn table() {
    let events = load_trace(&trace_dir().join("trace.jsonl")).expect("load trace");
    let rendered = render_table(&events, Zone::Utc, false);
    let out: Vec<&str> = rendered.lines().collect();
    assert_eq!(
        out[0].split_whitespace().collect::<Vec<_>>(),
        vec!["TIME", "STEP", "KIND", "ACTION", "DECISION", "OK", "TEXT"]
    );
    assert!(out[2].starts_with("13:13:20.000"));
    assert!(out[2].contains("PhoneTaskStarted"));
    assert!(out[2].contains("打车回家"));
    assert!(out[3].contains("tap #12 \"目的地输入框\""));
    let executed = out[4];
    assert!(executed.contains("allow: no rule matched"));
    assert!(executed.contains("yes"));
    assert!(out[5].contains("type \"家\" → \"目的地\""));
    assert!(out[6].contains("tap (500,900) \"确认呼叫\""));
    assert!(out[6].contains("ask: confirmation word"));
    assert!(out[7].contains("not json"));
    assert!(out[8].contains("已停在确认呼叫"));
    // columns line up in display cells, CJK counted as 2
    let kind_col: Vec<usize> = [
        (out[3], "StepPlanned"),
        (out[4], "StepExecuted"),
        (out[6], "StepBlocked"),
    ]
    .iter()
    .map(|(line, kind)| display_width(&line[..line.find(kind).unwrap()]))
    .collect();
    assert_eq!(
        kind_col
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        1
    );
}

#[test]
fn footer() {
    let events = load_trace(&trace_dir().join("trace.jsonl")).expect("load trace");
    assert_eq!(
        trace_footer(&events, Some(&trace_dir())),
        format!(
            "7 events, finished: success, 1 screenshots in {}",
            trace_dir().display()
        )
    );
}

#[test]
fn truncation() {
    assert_eq!(truncate("abcdef", 4), "abc…");
    assert_eq!(truncate("中文中文", 5), "中文…");
    assert_eq!(truncate("ab", 4), "ab");
}

#[test]
fn action_summaries() {
    let cases = [
        (
            json!({"type": "swipe", "from": {"x": 1, "y": 2}, "to": {"x": 3, "y": 4}}),
            "swipe (1,2)→(3,4)",
        ),
        (
            json!({"type": "launch", "packageName": "com.a.b"}),
            "launch com.a.b",
        ),
        (
            json!({"type": "deeplink", "uri": "x://y"}),
            "deeplink x://y",
        ),
        (json!({"type": "wait", "millis": 500}), "wait 500ms"),
        (json!({"type": "back"}), "back"),
        (
            json!({"type": "ask_user", "question": "继续？", "takeover": true}),
            "ask_user \"继续？\" (takeover)",
        ),
        (
            json!({"type": "finish", "success": false, "summary": "no"}),
            "finish fail \"no\"",
        ),
        (
            json!({"type": "type", "text": "a\nb", "nodeId": 3, "submit": true}),
            "type \"a b\" → #3 +submit",
        ),
    ];
    for (action, expected) in cases {
        assert_eq!(summarize_action(Some(&action)), expected, "{action}");
    }
}

#[test]
fn threadtime() {
    let event = parse_line(
        "10-31 12:00:01.234  1234  1250 I Xiaoman: r1 StepPlanned \
         {\"step\": 3, \"action\": {\"type\": \"tap\", \"nodeId\": 12}}\n",
    )
    .expect("event");
    assert_eq!(
        (
            event.run_id.as_str(),
            event.kind.as_str(),
            event.time.as_deref()
        ),
        ("r1", "StepPlanned", Some("12:00:01.234"))
    );
    assert_eq!(event.data.as_ref().unwrap()["step"], json!(3));
    assert!(!event.terminal());
    assert_eq!(
        format_event(&event),
        "12:00:01.234 r1 StepPlanned  #3 tap #12"
    );
}

#[test]
fn brief_and_raw() {
    let event = parse_line("I/Xiaoman( 1234): r2 Failed {\"message\": \"boom\"}").expect("event");
    assert_eq!(
        (event.run_id.as_str(), event.kind.as_str()),
        ("r2", "Failed")
    );
    assert!(event.terminal());
    assert_eq!(event.success(), Some(false));
    assert_eq!(format_event(&event), "r2 Failed  | boom");

    let event =
        parse_line("r3 PhoneTaskFinished {\"success\": true, \"summary\": \"完成\", \"steps\": 4}")
            .expect("event");
    assert!(event.terminal());
    assert_eq!(event.success(), Some(true));
    assert!(format_event(&event).contains("ok=yes | 完成"));
}

#[test]
fn agent_event_style_fields() {
    let event = parse_line(
        "I Xiaoman: r1 StepExecuted {\"stepIndex\": 2, \"action\": {\"type\": \"back\"}, \
         \"result\": {\"ok\": false, \"message\": \"no window\"}}",
    )
    .expect("event");
    assert_eq!(
        format_event(&event),
        "r1 StepExecuted  #2 back ok=no | no window"
    );
}

#[test]
fn truncated_json_and_noise() {
    let event = parse_line("I Xiaoman: r1 Reply {\"text\": \"cut").expect("event");
    assert_eq!(event.data, None);
    assert_eq!(format_event(&event), "r1 Reply  {\"text\": \"cut");
    assert_eq!(parse_line("--------- beginning of main"), None);
    assert_eq!(parse_line("I Xiaoman: skills reloaded, 0 problems"), None);
    assert_eq!(parse_line(""), None);
}
