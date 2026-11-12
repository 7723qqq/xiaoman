use serde_json::json;
use xiaoman_api::{Action, Point};

#[test]
fn tap_round_trips() {
    let tap = Action::Tap {
        point: Some(Point::new(500, 900).unwrap()),
        node_id: None,
        target: Some("确认呼叫".into()),
        reason: Some("下单".into()),
    };
    let encoded = serde_json::to_string(&tap).unwrap();
    assert_eq!(
        encoded,
        r#"{"type":"tap","point":{"x":500,"y":900},"target":"确认呼叫","reason":"下单"}"#
    );
    let decoded: Action = serde_json::from_str(&encoded).unwrap();
    assert_eq!(tap, decoded);
}

#[test]
fn decodes_model_style_json_ignoring_unknown_fields() {
    let action: Action =
        serde_json::from_str(r#"{"type":"launch","packageName":"com.autonavi.minimap","extra":1}"#)
            .unwrap();
    assert_eq!(
        action,
        Action::LaunchApp {
            package_name: "com.autonavi.minimap".into(),
            target: None,
            reason: None,
        }
    );
}

#[test]
fn rejects_out_of_range_point_from_json() {
    let result: Result<Action, _> =
        serde_json::from_str(r#"{"type":"tap","point":{"x":1001,"y":0}}"#);
    assert!(result.is_err());
}

#[test]
fn applies_documented_defaults() {
    let action: Action = serde_json::from_str(r#"{"type":"long_press","nodeId":3}"#).unwrap();
    assert_eq!(
        action,
        Action::LongPress {
            point: None,
            node_id: Some(3),
            duration_ms: 800,
            target: None,
            reason: None,
        }
    );

    let action: Action =
        serde_json::from_str(r#"{"type":"type","text":"回家","submit":true}"#).unwrap();
    assert_eq!(
        action,
        Action::TypeText {
            text: "回家".into(),
            node_id: None,
            submit: true,
            target: None,
            reason: None,
        }
    );

    let action: Action =
        serde_json::from_str(r#"{"type":"deeplink","uri":"amapuri://route/plan/"}"#).unwrap();
    assert_eq!(
        action,
        Action::OpenDeeplink {
            uri: "amapuri://route/plan/".into(),
            package_name: None,
            target: None,
            reason: None,
        }
    );
}

#[test]
fn finish_and_ask_user_are_terminal() {
    let finish: Action =
        serde_json::from_str(r#"{"type":"finish","success":true,"summary":"完成"}"#).unwrap();
    assert!(finish.is_terminal());
    let ask: Action = serde_json::from_str(r#"{"type":"ask_user","question":"验证码？"}"#).unwrap();
    assert!(ask.is_terminal());
    let tap: Action = serde_json::from_str(r#"{"type":"back"}"#).unwrap();
    assert!(!tap.is_terminal());
}

#[test]
fn node_json_omits_false_flags() {
    let node = xiaoman_api::UiNode {
        id: 7,
        text: Some("确认".into()),
        description: None,
        resource_id: None,
        class_name: None,
        bounds: xiaoman_api::Bounds {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        },
        clickable: true,
        editable: false,
        scrollable: false,
        focused: false,
        password: false,
    };
    let encoded = serde_json::to_value(&node).unwrap();
    assert_eq!(
        encoded,
        json!({"id":7,"text":"确认","bounds":{"left":0,"top":0,"right":100,"bottom":100},"clickable":true})
    );
}

use std::fs;
use std::path::Path;

#[test]
fn every_action_fixture_matches_byte_for_byte() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/action");
    let mut cases = 0;
    for entry in fs::read_dir(&dir).expect("fixtures/action") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .to_string();
        let Some(case) = name.strip_suffix(".in.json") else {
            continue;
        };
        let input = fs::read_to_string(&path).expect("input");
        let expected = fs::read_to_string(dir.join(format!("{case}.out.json")))
            .unwrap_or_else(|_| panic!("{case}.out.json"));
        let action: Action =
            serde_json::from_str(&input).unwrap_or_else(|error| panic!("{case}: {error}"));
        assert_eq!(
            expected,
            serde_json::to_string(&action).expect("encode"),
            "{case}"
        );
        cases += 1;
    }
    assert!(
        cases >= 10,
        "expected the shared action fixtures, found {cases}"
    );
}
