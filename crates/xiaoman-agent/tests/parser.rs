use xiaoman_agent::{ParsedReply, parse};
use xiaoman_api::{Action, Point};

fn ok(text: &str) -> Action {
    match parse(text) {
        ParsedReply::Ok { action, .. } => action,
        ParsedReply::Error { message } => panic!("expected Ok, got error: {message}"),
    }
}

#[test]
fn documented_format() {
    let parsed = parse(
        r#"{"thought": "目的地输入框在顶部", "action": {"type": "tap", "nodeId": 12, "target": "目的地输入框", "reason": "先输入目的地"}}"#,
    );
    match parsed {
        ParsedReply::Ok { action, thought } => {
            assert_eq!(thought.as_deref(), Some("目的地输入框在顶部"));
            assert_eq!(
                action,
                Action::Tap {
                    point: None,
                    node_id: Some(12),
                    target: Some("目的地输入框".into()),
                    reason: Some("先输入目的地".into()),
                }
            );
        }
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[test]
fn fenced_json_with_prose_and_extra_keys() {
    let action = ok(
        "好的，下一步：\n```json\n{\"thought\":\"x {not json}\",\"confidence\":0.9,\"action\":{\"type\":\"back\",\"extra\":1}}\n```\n完毕",
    );
    assert_eq!(
        action,
        Action::Back {
            target: None,
            reason: None
        }
    );
}

#[test]
fn bare_action_and_synonyms() {
    assert_eq!(
        ok(r#"{"type": "click", "node_id": 3}"#),
        Action::Tap {
            point: None,
            node_id: Some(3),
            target: None,
            reason: None
        }
    );
    assert_eq!(
        ok(r#"{"type": "Tap", "id": "4"}"#),
        Action::Tap {
            point: None,
            node_id: Some(4),
            target: None,
            reason: None
        }
    );
    assert_eq!(
        ok(r#"{"action": {"type": "input", "text": "北京南站", "nodeId": 2}}"#),
        Action::TypeText {
            text: "北京南站".into(),
            node_id: Some(2),
            submit: false,
            target: None,
            reason: None,
        }
    );
    assert_eq!(
        ok(r#"{"action": {"type": "open_app", "package": "com.android.settings"}}"#),
        Action::LaunchApp {
            package_name: "com.android.settings".into(),
            target: None,
            reason: None,
        }
    );
    assert_eq!(
        ok(r#"{"action": {"type": "done", "summary": "完成了"}}"#),
        Action::Finish {
            success: true,
            summary: "完成了".into(),
            target: None,
            reason: None,
        }
    );
    assert_eq!(
        ok(r#"{"action": {"type": "takeover", "question": "验证码？"}}"#),
        Action::AskUser {
            question: "验证码？".into(),
            takeover: true,
            target: None,
            reason: None,
        }
    );
}

#[test]
fn action_as_string_discriminator() {
    assert_eq!(
        ok(r#"{"thought": "t", "action": "tap", "nodeId": 7}"#),
        Action::Tap {
            point: None,
            node_id: Some(7),
            target: None,
            reason: None
        }
    );
}

#[test]
fn point_forms_are_clamped() {
    assert_eq!(
        ok(r#"{"action": {"type": "tap", "point": [100, 200]}}"#),
        Action::Tap {
            point: Some(Point::new(100, 200).unwrap()),
            node_id: None,
            target: None,
            reason: None
        }
    );
    assert_eq!(
        ok(r#"{"action": {"type": "tap", "point": {"x": 1200.4, "y": -5}}}"#),
        Action::Tap {
            point: Some(Point::new(1000, 0).unwrap()),
            node_id: None,
            target: None,
            reason: None
        }
    );
    assert_eq!(
        ok(r#"{"action": {"type": "tap", "x": 300, "y": 400}}"#),
        Action::Tap {
            point: Some(Point::new(300, 400).unwrap()),
            node_id: None,
            target: None,
            reason: None
        }
    );
    assert_eq!(
        ok(r#"{"action": {"type": "tap", "coordinate": "(10, 20)"}}"#),
        Action::Tap {
            point: Some(Point::new(10, 20).unwrap()),
            node_id: None,
            target: None,
            reason: None
        }
    );

    let swipe =
        ok(r#"{"action": {"type": "swipe", "from": [500, 800], "to": {"x": 500, "y": 2000}}}"#);
    match swipe {
        Action::Swipe { from, to, .. } => {
            assert_eq!(from, Point::new(500, 800).unwrap());
            assert_eq!(to, Point::new(500, 1000).unwrap());
        }
        other => panic!("expected swipe, got {other:?}"),
    }

    assert!(matches!(
        ok(r#"{"action": {"type": "scroll", "direction": "up"}}"#),
        Action::Swipe { .. }
    ));
}

#[test]
fn errors() {
    assert!(matches!(
        parse("我觉得应该点击设置"),
        ParsedReply::Error { .. }
    ));
    assert!(matches!(
        parse(r#"{"action": {"type": "teleport"}}"#),
        ParsedReply::Error { .. }
    ));
    assert!(matches!(
        parse(r#"{"action": {"type": "tap"}}"#),
        ParsedReply::Error { .. }
    ));
    assert!(matches!(
        parse(r#"{"action": {"type": "tap", "nodeId": 1"#),
        ParsedReply::Error { .. }
    ));
    match parse(r#"{"action": {"type": "type"}}"#) {
        ParsedReply::Error { message } => assert!(message.contains("text"), "{message}"),
        other => panic!("expected error, got {other:?}"),
    }
}
