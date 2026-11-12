use xiaoman_agent::note_text::NoteText;
use xiaoman_agent::screen_text;
use xiaoman_api::{Action, Bounds, Note, Observation, Point, Relation, ScreenState, UiNode};

fn node(id: i32, text: Option<&str>, bounds: Bounds) -> UiNode {
    UiNode {
        id,
        text: text.map(str::to_string),
        description: None,
        resource_id: None,
        class_name: Some("android.widget.Button".into()),
        bounds,
        clickable: true,
        editable: false,
        scrollable: false,
        focused: false,
        password: false,
    }
}

fn screen(package: &str, nodes: Vec<UiNode>) -> ScreenState {
    let mut state = ScreenState::new(None, 1080, 2400, Some(package.to_string()));
    state.nodes = nodes;
    state
}

#[test]
fn node_line_renders_flags_and_short_class() {
    let mut item = node(
        3,
        Some("  确认\n 呼叫 "),
        Bounds {
            left: 1,
            top: 2,
            right: 3,
            bottom: 4,
        },
    );
    item.description = Some("desc".into());
    item.resource_id = Some("com.x:id/confirm".into());
    item.password = true;
    assert_eq!(
        screen_text::node_line(&item),
        "[3] Button \"确认 呼叫\" desc=desc res=confirm (1,2,3,4) clickable password"
    );
}

#[test]
fn describe_covers_each_action_shape() {
    assert_eq!(
        screen_text::describe(&Action::Tap {
            point: Some(Point::new(500, 900).unwrap()),
            node_id: None,
            target: Some("确认呼叫 按钮".into()),
            reason: None,
        }),
        "点击 确认呼叫 按钮"
    );
    assert_eq!(
        screen_text::describe(&Action::Tap {
            point: None,
            node_id: Some(7),
            target: None,
            reason: None
        }),
        "点击 节点 7"
    );
    assert_eq!(
        screen_text::describe(&Action::Swipe {
            from: Point::new(500, 800).unwrap(),
            to: Point::new(500, 200).unwrap(),
            duration_ms: 300,
            target: None,
            reason: None,
        }),
        "滑动 (500,800)→(500,200)"
    );
    assert_eq!(
        screen_text::describe(&Action::TypeText {
            text: "回家".into(),
            node_id: None,
            submit: false,
            target: Some("目的地".into()),
            reason: None,
        }),
        "输入「回家」 到 目的地"
    );
}

#[test]
fn path_label_prefers_target_then_node_text() {
    let state = screen(
        "com.sdu.didi.psnger",
        vec![node(
            1,
            Some("立即支付"),
            Bounds {
                left: 0,
                top: 0,
                right: 10,
                bottom: 10,
            },
        )],
    );
    let with_target = Action::Tap {
        point: None,
        node_id: Some(1),
        target: Some("支付按钮".into()),
        reason: None,
    };
    assert_eq!(
        screen_text::path_label(&with_target, Some(&state)),
        Some("支付按钮".to_string())
    );

    let by_node = Action::Tap {
        point: None,
        node_id: Some(1),
        target: None,
        reason: None,
    };
    assert_eq!(
        screen_text::path_label(&by_node, Some(&state)),
        Some("立即支付".to_string())
    );

    assert_eq!(
        screen_text::path_label(
            &Action::Wait {
                millis: 1000,
                target: None,
                reason: None
            },
            Some(&state)
        ),
        None
    );
}

#[test]
fn approval_screen_requires_identical_nodes_and_geometry() {
    let nodes = vec![node(
        1,
        Some("支付"),
        Bounds {
            left: 0,
            top: 0,
            right: 10,
            bottom: 10,
        },
    )];
    let before = screen("com.x", nodes.clone());
    let after = screen("com.x", nodes.clone());
    assert!(screen_text::matches_approval_screen(Some(&before), &after));

    let changed = screen(
        "com.x",
        vec![node(
            1,
            Some("支付"),
            Bounds {
                left: 0,
                top: 0,
                right: 11,
                bottom: 10,
            },
        )],
    );
    assert!(!screen_text::matches_approval_screen(
        Some(&before),
        &changed
    ));
    assert!(!screen_text::matches_approval_screen(None, &after));
}

#[test]
fn note_body_and_preamble_round_trip() {
    let observations = vec![Observation {
        category: "address".into(),
        content: "北京市海淀区".into(),
        tags: vec!["常用".into()],
        context: None,
    }];
    let relations = vec![Relation {
        relation_type: "lives_in".into(),
        target: "北京".into(),
        context: None,
    }];
    let body = NoteText::body(
        "家",
        &observations,
        &relations,
        "# 家\n\n前言文字。\n\n## Observations\n- [old] x\n",
    );
    assert!(body.starts_with("# 家\n\n前言文字。\n"), "{body}");
    assert!(body.contains("- [address] 北京市海淀区 #常用"), "{body}");
    assert!(body.contains("- lives_in [[北京]]"), "{body}");

    assert_eq!(
        NoteText::preamble("# 家\n\n前言文字。\n\n## Observations\n- [a] b\n"),
        "# 家\n\n前言文字。\n\n"
    );

    let note = Note {
        title: "家".into(),
        permalink: "places/家".into(),
        note_type: "place".into(),
        tags: vec![],
        body: body.clone(),
        observations,
        relations,
        extra_frontmatter: Default::default(),
        updated_at_millis: 0,
    };
    let prompt = NoteText::for_prompt(&note, 1500);
    assert!(prompt.starts_with("### 家 (places/家)\n"), "{prompt}");
    assert!(
        prompt.contains("- [address] 北京市海淀区 #常用"),
        "{prompt}"
    );
    assert_eq!(
        NoteText::for_prompt(&note, 10),
        format!("{}…", prompt.chars().take(10).collect::<String>())
    );
}
