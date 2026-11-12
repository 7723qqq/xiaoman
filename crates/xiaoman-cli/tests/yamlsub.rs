use serde_json::json;
use xiaoman_cli::yamlsub::{Value, parse, parse_with, split_frontmatter, to_plain};

fn plain(text: &str) -> serde_json::Value {
    to_plain(&parse(text).expect("parse"))
}

#[test]
fn types() {
    assert_eq!(
        plain("a: 1\nb: 1.5\nc: true\nd: null\ne: ~\nf:\ng: hello world"),
        json!({"a": 1, "b": 1.5, "c": true, "d": null, "e": null, "f": null, "g": "hello world"})
    );
}

#[test]
fn quoted() {
    assert_eq!(
        plain("a: \"x \\\"y\\\" \\u4e2d\\n\"\nb: 'it''s'\nc: \"1\""),
        json!({"a": "x \"y\" 中\n", "b": "it's", "c": "1"})
    );
}

#[test]
fn raw_text_kept() {
    let root = parse("wait: 1500\nq: '1500'\nn:").expect("parse");
    assert_eq!(root.get("wait").unwrap().raw.as_deref(), Some("1500"));
    assert_eq!(root.get("wait").unwrap().value, Value::Int(1500));
    assert_eq!(root.get("q").unwrap().raw.as_deref(), Some("1500"));
    assert_eq!(root.get("n").unwrap().raw, None);
}

#[test]
fn comments() {
    assert_eq!(
        plain("name: taxi   # comment\nurl: 'a#b'\nx: a#b\n# whole line\ny: \"#\""),
        json!({"name": "taxi", "url": "a#b", "x": "a#b", "y": "#"})
    );
}

#[test]
fn uri_values() {
    assert_eq!(
        plain("search: \"amapuri://route/plan/?dname={destination}\"\nraw: amapuri://x?a=1"),
        json!({"search": "amapuri://route/plan/?dname={destination}", "raw": "amapuri://x?a=1"})
    );
}

#[test]
fn colon_space_in_plain_value_is_error() {
    assert!(parse("description: a: b").is_err());
}

#[test]
fn cjk_colon_is_fine() {
    assert_eq!(
        plain("description: 打车：回家"),
        json!({"description": "打车：回家"})
    );
}

#[test]
fn multiline_plain_folds() {
    assert_eq!(
        plain("description: one\n  two\nname: x"),
        json!({"description": "one two", "name": "x"})
    );
}

#[test]
fn block_scalars() {
    assert_eq!(
        plain("a: |\n  l1\n  l2\nb: >-\n  f1\n  f2\nc: x"),
        json!({"a": "l1\nl2\n", "b": "f1 f2", "c": "x"})
    );
}

#[test]
fn inline_and_block_lists() {
    assert_eq!(
        plain(
            "apps: [com.a.b, 'x y', \"中\"]\nmem:\n  - 地址/家\n  - 偏好/车型\nsame:\n- a\n- b\nempty: []"
        ),
        json!({
            "apps": ["com.a.b", "x y", "中"],
            "mem": ["地址/家", "偏好/车型"],
            "same": ["a", "b"],
            "empty": []
        })
    );
}

#[test]
fn one_level_map() {
    assert_eq!(
        plain("deeplinks:\n  search: \"amapuri://a\"\n  home: b://c\nnext: 1"),
        json!({"deeplinks": {"search": "amapuri://a", "home": "b://c"}, "next": 1})
    );
}

#[test]
fn flow_steps() {
    let text = "steps:\n  - launch: com.autonavi.minimap\n  - type: { target: 目的地, text: \"{destination}\" }\n  - wait: 1500\n  - type:\n      target: x\n      text: y\n";
    assert_eq!(
        plain(text),
        json!({"steps": [
            {"launch": "com.autonavi.minimap"},
            {"type": {"target": "目的地", "text": "{destination}"}},
            {"wait": 1500},
            {"type": {"target": "x", "text": "y"}}
        ]})
    );
}

#[test]
fn unquoted_brace_is_flow_mapping() {
    assert_eq!(
        plain("text: {destination}"),
        json!({"text": {"destination": null}})
    );
}

#[test]
fn nested_flow() {
    assert_eq!(
        plain("a: [1, [2, 3], {k: v}]"),
        json!({"a": [1, [2, 3], {"k": "v"}]})
    );
}

#[test]
fn multi_key_seq_item() {
    assert_eq!(plain("- a: 1\n  b: 2\n- c"), json!([{"a": 1, "b": 2}, "c"]));
}

#[test]
fn line_numbers() {
    let root =
        parse_with("a: 1\n\n# c\nsteps:\n  - tap: x\n  - wait: 2\n", 1, None).expect("parse");
    assert_eq!(root.get("a").unwrap().line, 2);
    let steps = root.get("steps").unwrap();
    assert_eq!(steps.key_line, 5);
    let items = steps.items().unwrap();
    assert_eq!(
        items.iter().map(|step| step.line).collect::<Vec<_>>(),
        vec![6, 7]
    );
    assert_eq!(items[1].get("wait").unwrap().key_line, 7);
}

#[test]
fn errors() {
    for bad in [
        "a: [1, 2",
        "a: {x: 1",
        "a: 'x",
        "a:\n\t- b",
        "a: &x 1",
        "a: 1\n   b: 2",
    ] {
        assert!(parse(bad).is_err(), "expected an error for {bad:?}");
    }
}

#[test]
fn error_line() {
    let error = parse_with("a: 1\nb: [x\n", 1, None).unwrap_err();
    assert_eq!(error.line, 3);
}

#[test]
fn duplicates() {
    assert!(parse("a: 1\na: 2").is_err());
    let mut duplicates = Vec::new();
    let root = parse_with("a: 1\nb: {x: 1, x: 2}\na: 2", 0, Some(&mut duplicates)).expect("parse");
    assert_eq!(to_plain(&root), json!({"a": 1, "b": {"x": 1}}));
    assert_eq!(duplicates, vec![("x".to_string(), 2), ("a".to_string(), 3)]);
}

#[test]
fn empty() {
    assert_eq!(plain(""), json!(null));
    assert_eq!(plain("# only a comment\n"), json!(null));
}

#[test]
fn frontmatter_split() {
    let (frontmatter, first, body) =
        split_frontmatter("\u{feff}---\nname: x\n---\n# Body\n").expect("split");
    assert_eq!(frontmatter, "name: x");
    assert_eq!(first, 2);
    assert_eq!(body, "# Body");
}

#[test]
fn frontmatter_missing() {
    assert!(split_frontmatter("name: x\n").is_err());
    assert!(split_frontmatter("---\nname: x\n").is_err());
}
