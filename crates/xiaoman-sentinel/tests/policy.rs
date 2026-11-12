use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoman_api::{
    Action, ApprovalAnswer, ApprovalError, ApprovalGateway, ApprovalRequest, ApprovalScope,
    AuditRecord, AuditSink, Bounds, Decision, GuardContext, Point, ScreenState, Sentinel, Skill,
    SkillChannel, UiNode,
};
use xiaoman_sentinel::{PolicySentinel, SentinelOptions};

struct FakeGateway {
    answer: Mutex<ApprovalAnswer>,
    requests: Mutex<Vec<ApprovalRequest>>,
}

impl FakeGateway {
    fn new() -> Self {
        Self {
            answer: Mutex::new(ApprovalAnswer::once(true)),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn answer(&self, answer: ApprovalAnswer) {
        *self.answer.lock().unwrap() = answer;
    }

    fn requests(&self) -> Vec<ApprovalRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[async_trait]
impl ApprovalGateway for FakeGateway {
    async fn request(&self, request: ApprovalRequest) -> Result<ApprovalAnswer, ApprovalError> {
        self.requests.lock().unwrap().push(request);
        Ok(*self.answer.lock().unwrap())
    }
}

struct FailingGateway;

#[async_trait]
impl ApprovalGateway for FailingGateway {
    async fn request(&self, _request: ApprovalRequest) -> Result<ApprovalAnswer, ApprovalError> {
        Err(ApprovalError("ui gone".into()))
    }
}

#[derive(Default)]
struct RecordingAudit(Mutex<Vec<AuditRecord>>);

impl AuditSink for RecordingAudit {
    fn record(&self, record: AuditRecord) {
        self.0.lock().unwrap().push(record);
    }
}

impl RecordingAudit {
    fn records(&self) -> Vec<AuditRecord> {
        self.0.lock().unwrap().clone()
    }
}

struct Fixture {
    gateway: Arc<FakeGateway>,
    audit: Arc<RecordingAudit>,
    sentinel: PolicySentinel,
}

fn fixture() -> Fixture {
    let gateway = Arc::new(FakeGateway::new());
    let audit = Arc::new(RecordingAudit::default());
    let sentinel = PolicySentinel::with_options(
        SentinelOptions {
            clock: Arc::new(|| 42i64),
            ..Default::default()
        },
        gateway.clone(),
        audit.clone(),
    );
    Fixture {
        gateway,
        audit,
        sentinel,
    }
}

fn didi() -> Skill {
    Skill {
        name: "taxi-home".into(),
        description: "打车回家".into(),
        apps: vec!["com.sdu.didi.psnger".into(), "com.autonavi.minimap".into()],
        channel: SkillChannel::Gui,
        confirm_before: vec!["确认叫车".into()],
        memory_refs: vec![],
        deeplinks: vec![(
            "search".into(),
            "amapuri://route/plan/?dname={destination}".into(),
        )],
        instructions: String::new(),
        flow: None,
        source: "test".into(),
    }
}

fn ui_node(id: i32) -> UiNode {
    UiNode {
        id,
        text: None,
        description: None,
        resource_id: None,
        class_name: None,
        bounds: Bounds {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 1000,
        },
        clickable: false,
        editable: false,
        scrollable: false,
        focused: false,
        password: false,
    }
}

fn text(mut node: UiNode, value: &str) -> UiNode {
    node.text = Some(value.into());
    node
}

fn desc(mut node: UiNode, value: &str) -> UiNode {
    node.description = Some(value.into());
    node
}

fn bounds(mut node: UiNode, left: i32, top: i32, right: i32, bottom: i32) -> UiNode {
    node.bounds = Bounds {
        left,
        top,
        right,
        bottom,
    };
    node
}

fn editable(mut node: UiNode) -> UiNode {
    node.editable = true;
    node
}

fn focused(mut node: UiNode) -> UiNode {
    node.focused = true;
    node
}

fn password(mut node: UiNode) -> UiNode {
    node.password = true;
    node
}

fn screen_of(pkg: Option<&str>, nodes: Vec<UiNode>, secure: bool) -> Arc<ScreenState> {
    let mut state = ScreenState::new(None, 1080, 2400, pkg.map(str::to_string));
    state.nodes = nodes;
    state.secure = secure;
    Arc::new(state)
}

fn default_screen() -> Arc<ScreenState> {
    screen_of(Some("com.sdu.didi.psnger"), vec![], false)
}

fn ctx(
    skill: Option<Skill>,
    screen: Option<Arc<ScreenState>>,
    task: &str,
    step: i32,
) -> GuardContext {
    GuardContext {
        task_id: task.into(),
        goal: "goal".into(),
        skill: skill.map(Arc::new),
        screen,
        step_index: step,
    }
}

fn point(x: i32, y: i32) -> Point {
    Point::new(x, y).unwrap()
}

fn tap(point_at: Option<Point>, node_id: Option<i32>, target: Option<&str>) -> Action {
    Action::Tap {
        point: point_at,
        node_id,
        target: target.map(str::to_string),
        reason: None,
    }
}

fn long_press(node_id: Option<i32>, target: Option<&str>) -> Action {
    Action::LongPress {
        point: None,
        node_id,
        duration_ms: 800,
        target: target.map(str::to_string),
        reason: None,
    }
}

fn type_text(value: &str, node_id: Option<i32>, submit: bool, target: Option<&str>) -> Action {
    Action::TypeText {
        text: value.into(),
        node_id,
        submit,
        target: target.map(str::to_string),
        reason: None,
    }
}

fn swipe(from: Point, to: Point) -> Action {
    Action::Swipe {
        from,
        to,
        duration_ms: 300,
        target: None,
        reason: None,
    }
}

fn launch(package: &str) -> Action {
    Action::LaunchApp {
        package_name: package.into(),
        target: None,
        reason: None,
    }
}

fn deeplink(uri: &str, package: Option<&str>) -> Action {
    Action::OpenDeeplink {
        uri: uri.into(),
        package_name: package.map(str::to_string),
        target: None,
        reason: None,
    }
}

fn back() -> Action {
    Action::Back {
        target: None,
        reason: None,
    }
}

fn home() -> Action {
    Action::Home {
        target: None,
        reason: None,
    }
}

fn wait() -> Action {
    Action::Wait {
        millis: 1000,
        target: None,
        reason: None,
    }
}

fn ask_user(question: &str) -> Action {
    Action::AskUser {
        question: question.into(),
        takeover: false,
        target: None,
        reason: None,
    }
}

fn finish(summary: &str) -> Action {
    Action::Finish {
        success: true,
        summary: summary.into(),
        target: None,
        reason: None,
    }
}

fn ask_prompt(decision: &Decision) -> &str {
    match decision {
        Decision::Ask { prompt, .. } => prompt,
        other => panic!("expected Ask, got {other:?}"),
    }
}

fn is_deny(decision: &Decision) -> bool {
    matches!(decision, Decision::Deny { .. })
}

fn is_allow(decision: &Decision) -> bool {
    matches!(decision, Decision::Allow { .. })
}

// ---- deny ----

#[test]
fn denies_typing_into_password_node_by_id() {
    let f = fixture();
    let screen = screen_of(
        Some("com.sdu.didi.psnger"),
        vec![
            text(editable(ui_node(0)), "账号"),
            text(password(editable(ui_node(1))), "密码"),
        ],
        false,
    );
    let context = ctx(Some(didi()), Some(screen), "t1", 0);
    pollster::block_on(async {
        assert!(is_deny(
            &f.sentinel
                .guard(&context, &type_text("secret", Some(1), false, None))
                .await
        ));
        assert!(is_allow(
            &f.sentinel
                .guard(&context, &type_text("me", Some(0), false, None))
                .await
        ));
    });
}

#[test]
fn denies_typing_into_focused_password_node() {
    let f = fixture();
    let screen = screen_of(
        Some("com.sdu.didi.psnger"),
        vec![
            editable(ui_node(0)),
            password(focused(editable(ui_node(1)))),
        ],
        false,
    );
    let context = ctx(Some(didi()), Some(screen), "t1", 0);
    pollster::block_on(async {
        let decision = f
            .sentinel
            .guard(&context, &type_text("secret", None, false, None))
            .await;
        assert!(is_deny(&decision));
        assert!(decision.reason().contains("password"));
    });
}

#[test]
fn secure_screen_allows_only_navigation() {
    let f = fixture();
    let context = ctx(
        Some(didi()),
        Some(screen_of(Some("com.sdu.didi.psnger"), vec![], true)),
        "t1",
        0,
    );
    pollster::block_on(async {
        assert!(is_deny(
            &f.sentinel
                .guard(&context, &tap(Some(point(10, 10)), None, None))
                .await
        ));
        assert!(is_deny(
            &f.sentinel
                .guard(&context, &swipe(point(10, 10), point(10, 500)))
                .await
        ));
        assert!(is_deny(
            &f.sentinel
                .guard(&context, &launch("com.sdu.didi.psnger"))
                .await
        ));
        let decision = f
            .sentinel
            .guard(&context, &type_text("x", None, false, None))
            .await;
        assert!(is_deny(&decision));
        assert!(decision.reason().contains("take over"));
        assert!(is_allow(&f.sentinel.guard(&context, &back()).await));
        assert!(is_allow(&f.sentinel.guard(&context, &home()).await));
        assert!(is_allow(&f.sentinel.guard(&context, &wait()).await));
        assert!(is_allow(
            &f.sentinel.guard(&context, &ask_user("请接管")).await
        ));
    });
}

#[test]
fn launch_outside_skill_apps_is_denied() {
    let f = fixture();
    let base = ctx(Some(didi()), Some(default_screen()), "t1", 0);
    let no_apps = ctx(
        Some({
            let mut skill = didi();
            skill.apps.clear();
            skill
        }),
        Some(default_screen()),
        "t1",
        0,
    );
    let no_skill = ctx(None, Some(default_screen()), "t1", 0);
    pollster::block_on(async {
        assert!(is_deny(
            &f.sentinel.guard(&base, &launch("com.tencent.mm")).await
        ));
        assert!(is_allow(
            &f.sentinel
                .guard(&base, &launch("com.autonavi.minimap"))
                .await
        ));
        assert!(is_allow(
            &f.sentinel
                .guard(&base, &launch("com.android.launcher3"))
                .await
        ));
        assert!(is_allow(
            &f.sentinel.guard(&base, &launch("dev.xiaoman.app")).await
        ));
        assert!(is_allow(
            &f.sentinel.guard(&no_apps, &launch("com.tencent.mm")).await
        ));
        assert!(is_allow(
            &f.sentinel.guard(&no_skill, &launch("com.tencent.mm")).await
        ));
    });
}

#[test]
fn deeplink_to_foreign_package_is_denied() {
    let f = fixture();
    let context = ctx(Some(didi()), Some(default_screen()), "t1", 0);
    pollster::block_on(async {
        assert!(is_deny(
            &f.sentinel
                .guard(
                    &context,
                    &deeplink("amapuri://route/plan/?dname=家", Some("com.tencent.mm"))
                )
                .await
        ));
    });
}

#[test]
fn touch_on_foreign_screen_is_denied() {
    let f = fixture();
    let foreign = ctx(
        Some(didi()),
        Some(screen_of(Some("com.tencent.mm"), vec![], false)),
        "t1",
        0,
    );
    let launcher = ctx(
        Some(didi()),
        Some(screen_of(Some("com.miui.home"), vec![], false)),
        "t1",
        0,
    );
    let own = ctx(
        Some(didi()),
        Some(screen_of(Some("dev.xiaoman.app"), vec![], false)),
        "t1",
        0,
    );
    let system_ui = ctx(
        Some(didi()),
        Some(screen_of(Some("com.android.systemui"), vec![], false)),
        "t1",
        0,
    );
    pollster::block_on(async {
        assert!(is_deny(
            &f.sentinel
                .guard(&foreign, &tap(Some(point(1, 1)), None, None))
                .await
        ));
        assert!(is_deny(
            &f.sentinel.guard(&foreign, &long_press(Some(0), None)).await
        ));
        assert!(is_deny(
            &f.sentinel
                .guard(&foreign, &type_text("hi", None, false, None))
                .await
        ));
        assert!(is_allow(&f.sentinel.guard(&foreign, &back()).await));
        assert!(is_allow(
            &f.sentinel
                .guard(&launcher, &tap(Some(point(1, 1)), None, None))
                .await
        ));
        assert!(is_allow(
            &f.sentinel
                .guard(&own, &tap(Some(point(1, 1)), None, None))
                .await
        ));
        assert!(is_deny(
            &f.sentinel
                .guard(&system_ui, &tap(Some(point(1, 1)), None, None))
                .await
        ));
        assert!(is_allow(&f.sentinel.guard(&system_ui, &home()).await));
    });
}

#[test]
fn custom_launcher_and_self_package() {
    let gateway = Arc::new(FakeGateway::new());
    let audit = Arc::new(RecordingAudit::default());
    let custom = PolicySentinel::with_options(
        SentinelOptions {
            launcher_packages: ["my.launcher".to_string()].into_iter().collect(),
            self_package: "my.self".into(),
            ..Default::default()
        },
        gateway,
        audit,
    );
    let context = ctx(Some(didi()), Some(default_screen()), "t1", 0);
    pollster::block_on(async {
        assert!(is_allow(
            &custom.guard(&context, &launch("my.launcher")).await
        ));
        assert!(is_allow(&custom.guard(&context, &launch("my.self")).await));
        assert!(is_deny(
            &custom.guard(&context, &launch("dev.xiaoman.app")).await
        ));
    });
}

// ---- ask: confirmation words ----

#[test]
fn asks_for_confirmation_word_in_target() {
    let f = fixture();
    let context = ctx(Some(didi()), Some(default_screen()), "t1", 0);
    pollster::block_on(async {
        let decision = f
            .sentinel
            .guard(
                &context,
                &tap(Some(point(500, 900)), None, Some("确认呼叫 按钮")),
            )
            .await;
        assert_eq!(
            ask_prompt(&decision),
            "即将点击「确认呼叫 按钮」，是否继续？"
        );
        assert!(decision.reason().contains("确认呼叫"));
    });
}

#[test]
fn latin_words_match_case_insensitively_at_word_boundaries() {
    let f = fixture();
    let context = ctx(Some(didi()), Some(default_screen()), "t1", 0);
    pollster::block_on(async {
        for text in ["Place ORDER", "Send", "点击send按钮"] {
            assert!(
                !is_allow(
                    &f.sentinel
                        .guard(&context, &tap(Some(point(1, 1)), None, Some(text)))
                        .await
                ),
                "{text}"
            );
        }
        for text in ["border", "sender name", "Payment history", "搜索"] {
            assert!(
                is_allow(
                    &f.sentinel
                        .guard(&context, &tap(Some(point(1, 1)), None, Some(text)))
                        .await
                ),
                "{text}"
            );
        }
    });
}

#[test]
fn resolves_node_by_id() {
    let f = fixture();
    let screen = screen_of(
        Some("com.sdu.didi.psnger"),
        vec![text(ui_node(0), "首页"), desc(ui_node(7), "删除")],
        false,
    );
    let context = ctx(Some(didi()), Some(screen), "t1", 0);
    pollster::block_on(async {
        let decision = f.sentinel.guard(&context, &long_press(Some(7), None)).await;
        assert_eq!(ask_prompt(&decision), "即将长按「删除」，是否继续？");
        assert!(is_allow(
            &f.sentinel.guard(&context, &tap(None, Some(0), None)).await
        ));
    });
}

#[test]
fn resolves_smallest_node_containing_point() {
    let f = fixture();
    let screen = screen_of(
        Some("com.sdu.didi.psnger"),
        vec![
            text(bounds(ui_node(0), 0, 0, 1000, 1000), "页面"),
            text(bounds(ui_node(1), 0, 800, 1000, 1000), "底栏"),
            text(bounds(ui_node(2), 600, 850, 950, 950), "立即支付"),
        ],
        false,
    );
    let context = ctx(Some(didi()), Some(screen), "t1", 0);
    pollster::block_on(async {
        let decision = f
            .sentinel
            .guard(&context, &tap(Some(point(700, 900)), None, None))
            .await;
        assert_eq!(ask_prompt(&decision), "即将点击「立即支付」，是否继续？");
        assert!(decision.reason().contains("立即支付"));
        assert!(is_allow(
            &f.sentinel
                .guard(&context, &tap(Some(point(100, 900)), None, None))
                .await
        ));
        assert!(is_allow(
            &f.sentinel
                .guard(&context, &tap(Some(point(100, 100)), None, None))
                .await
        ));
    });
}

#[test]
fn type_checks_target_and_field_but_not_typed_text() {
    let f = fixture();
    let plain = ctx(Some(didi()), Some(default_screen()), "t1", 0);
    let screen = screen_of(
        Some("com.sdu.didi.psnger"),
        vec![desc(focused(editable(ui_node(3))), "发送消息")],
        false,
    );
    let field = ctx(Some(didi()), Some(screen), "t1", 0);
    pollster::block_on(async {
        assert!(is_allow(
            &f.sentinel
                .guard(&plain, &type_text("请删除这条", None, true, None))
                .await
        ));
        let decision = f
            .sentinel
            .guard(&field, &type_text("你好", None, true, None))
            .await;
        assert_eq!(
            ask_prompt(&decision),
            "即将在「发送消息」输入并提交，是否继续？"
        );
        assert!(!is_allow(
            &f.sentinel
                .guard(&plain, &type_text("hi", None, false, Some("send box")))
                .await
        ));
    });
}

#[test]
fn skill_and_extra_words_add_to_built_ins() {
    let f = fixture();
    let with_skill = ctx(Some(didi()), Some(default_screen()), "t1", 0);
    let without_skill = ctx(None, Some(default_screen()), "t1", 0);
    let gateway = Arc::new(FakeGateway::new());
    let audit = Arc::new(RecordingAudit::default());
    let extra = PolicySentinel::with_options(
        SentinelOptions {
            extra_confirm_words: vec!["Checkout".into()],
            ..Default::default()
        },
        gateway,
        audit,
    );
    let bare = ctx(
        Some({
            let mut skill = didi();
            skill.confirm_before = vec!["".into(), " ".into()];
            skill
        }),
        Some(default_screen()),
        "t1",
        0,
    );
    pollster::block_on(async {
        assert!(!is_allow(
            &f.sentinel
                .guard(&with_skill, &tap(Some(point(1, 1)), None, Some("确认叫车")))
                .await
        ));
        assert!(is_allow(
            &f.sentinel
                .guard(
                    &without_skill,
                    &tap(Some(point(1, 1)), None, Some("确认叫车"))
                )
                .await
        ));
        assert!(!is_allow(
            &extra
                .guard(
                    &with_skill,
                    &tap(Some(point(1, 1)), None, Some("checkout now"))
                )
                .await
        ));
        assert!(!is_allow(
            &f.sentinel
                .guard(&bare, &tap(Some(point(1, 1)), None, Some("支付")))
                .await
        ));
    });
    let built_ins: Vec<&str> = xiaoman_sentinel::BUILT_IN_CONFIRM_WORDS.to_vec();
    assert!(built_ins.contains(&"支付"));
    assert!(built_ins.contains(&"order"));
}

// ---- ask: deeplinks ----

#[test]
fn deeplink_matching_skill_template_is_allowed() {
    let f = fixture();
    let context = ctx(Some(didi()), Some(default_screen()), "t1", 0);
    pollster::block_on(async {
        assert!(is_allow(
            &f.sentinel
                .guard(&context, &deeplink("amapuri://route/plan/?dname=家", None))
                .await
        ));
        assert!(is_allow(
            &f.sentinel
                .guard(&context, &deeplink("AMAPURI://Route/other", None))
                .await
        ));
        assert!(!is_allow(
            &f.sentinel
                .guard(&context, &deeplink("amapuri://navi/?x=1", None))
                .await
        ));
        assert!(!is_allow(
            &f.sentinel
                .guard(&context, &deeplink("didi://route/plan", None))
                .await
        ));
        assert!(!is_allow(
            &f.sentinel
                .guard(&context, &deeplink("not a uri", None))
                .await
        ));
    });
}

#[test]
fn deeplink_without_skill_or_templates_asks() {
    let f = fixture();
    let no_skill = ctx(None, Some(default_screen()), "t1", 0);
    let no_templates = ctx(
        Some({
            let mut skill = didi();
            skill.deeplinks.clear();
            skill
        }),
        Some(default_screen()),
        "t1",
        0,
    );
    pollster::block_on(async {
        let decision = f
            .sentinel
            .guard(&no_skill, &deeplink("amapuri://route/plan/?dname=家", None))
            .await;
        assert!(ask_prompt(&decision).starts_with("即将打开链接"));
        assert!(!is_allow(
            &f.sentinel
                .guard(&no_templates, &deeplink("amapuri://route/plan", None))
                .await
        ));
    });
}

#[test]
fn plain_actions_are_allowed() {
    let f = fixture();
    let context = ctx(Some(didi()), Some(default_screen()), "t1", 0);
    let no_screen = ctx(Some(didi()), None, "t1", 0);
    pollster::block_on(async {
        assert!(is_allow(
            &f.sentinel
                .guard(&context, &swipe(point(500, 800), point(500, 200)))
                .await
        ));
        assert!(is_allow(
            &f.sentinel
                .guard(&no_screen, &tap(Some(point(1, 1)), None, Some("搜索")))
                .await
        ));
        assert!(is_allow(&f.sentinel.guard(&context, &finish("done")).await));
    });
}

// ---- authorize ----

#[test]
fn authorize_audits_allow_and_deny_without_asking() {
    let f = fixture();
    pollster::block_on(async {
        assert!(
            f.sentinel
                .authorize(&ctx(Some(didi()), Some(default_screen()), "t1", 3), &back())
                .await
        );
        assert!(
            !f.sentinel
                .authorize(
                    &ctx(Some(didi()), Some(default_screen()), "t1", 4),
                    &launch("com.tencent.mm")
                )
                .await
        );
    });
    assert_eq!(f.gateway.request_count(), 0);
    let records = f.audit.records();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0],
        AuditRecord {
            time_millis: 42,
            task_id: "t1".into(),
            step_index: 3,
            action: back(),
            decision: Decision::allow(),
            approved: None,
        }
    );
    assert!(is_deny(&records[1].decision));
    assert_eq!(records[1].approved, None);
    assert_eq!(records[1].step_index, 4);
}

#[test]
fn once_approval_asks_every_time() {
    let f = fixture();
    let context = ctx(Some(didi()), Some(default_screen()), "t1", 0);
    let action = tap(Some(point(1, 1)), None, Some("确认呼叫"));
    pollster::block_on(async {
        assert!(f.sentinel.authorize(&context, &action).await);
        assert!(f.sentinel.authorize(&context, &action).await);
    });
    assert_eq!(f.gateway.request_count(), 2);
    let requests = f.gateway.requests();
    assert_eq!(requests[0].prompt, "即将点击「确认呼叫」，是否继续？");
    assert_eq!(requests[0].task_id, "t1");
    assert_eq!(
        f.audit
            .records()
            .into_iter()
            .map(|record| record.approved)
            .collect::<Vec<_>>(),
        vec![Some(true), Some(true)]
    );
}

#[test]
fn task_approval_is_reused_for_same_word_and_task_only() {
    let f = fixture();
    f.gateway.answer(ApprovalAnswer {
        approved: true,
        scope: ApprovalScope::Task,
    });
    pollster::block_on(async {
        assert!(
            f.sentinel
                .authorize(
                    &ctx(Some(didi()), Some(default_screen()), "t1", 0),
                    &tap(Some(point(1, 1)), None, Some("发送"))
                )
                .await
        );
        f.gateway.answer(ApprovalAnswer::once(false));
        assert!(
            f.sentinel
                .authorize(
                    &ctx(Some(didi()), Some(default_screen()), "t1", 1),
                    &tap(Some(point(1, 1)), None, Some("发送 按钮"))
                )
                .await
        );
    });
    assert_eq!(f.gateway.request_count(), 1);
    pollster::block_on(async {
        assert!(
            !f.sentinel
                .authorize(
                    &ctx(Some(didi()), Some(default_screen()), "t1", 2),
                    &tap(Some(point(1, 1)), None, Some("删除"))
                )
                .await
        );
        assert!(
            !f.sentinel
                .authorize(
                    &ctx(Some(didi()), Some(default_screen()), "t2", 3),
                    &tap(Some(point(1, 1)), None, Some("发送"))
                )
                .await
        );
    });
    assert_eq!(f.gateway.request_count(), 3);
    assert_eq!(
        f.audit
            .records()
            .into_iter()
            .map(|record| record.approved)
            .collect::<Vec<_>>(),
        vec![Some(true), Some(true), Some(false), Some(false)]
    );
    assert!(
        f.audit
            .records()
            .iter()
            .all(|record| matches!(record.decision, Decision::Ask { .. }))
    );
}

#[test]
fn end_task_forgets_task_approvals() {
    let f = fixture();
    f.gateway.answer(ApprovalAnswer {
        approved: true,
        scope: ApprovalScope::Task,
    });
    let action = tap(Some(point(1, 1)), None, Some("支付"));
    pollster::block_on(async {
        assert!(
            f.sentinel
                .authorize(&ctx(Some(didi()), Some(default_screen()), "t1", 0), &action)
                .await
        );
        f.sentinel.end_task("t1");
        f.gateway.answer(ApprovalAnswer::once(false));
        assert!(
            !f.sentinel
                .authorize(&ctx(Some(didi()), Some(default_screen()), "t1", 1), &action)
                .await
        );
    });
    assert_eq!(f.gateway.request_count(), 2);
}

#[test]
fn denied_task_scope_is_not_remembered() {
    let f = fixture();
    f.gateway.answer(ApprovalAnswer {
        approved: false,
        scope: ApprovalScope::Task,
    });
    let action = tap(Some(point(1, 1)), None, Some("支付"));
    pollster::block_on(async {
        assert!(
            !f.sentinel
                .authorize(&ctx(Some(didi()), Some(default_screen()), "t1", 0), &action)
                .await
        );
        assert!(
            !f.sentinel
                .authorize(&ctx(Some(didi()), Some(default_screen()), "t1", 1), &action)
                .await
        );
    });
    assert_eq!(f.gateway.request_count(), 2);
}

#[test]
fn gateway_failure_means_not_approved() {
    let audit = Arc::new(RecordingAudit::default());
    let sentinel = PolicySentinel::with_options(
        SentinelOptions {
            clock: Arc::new(|| 42i64),
            ..Default::default()
        },
        Arc::new(FailingGateway),
        audit.clone(),
    );
    let context = ctx(Some(didi()), Some(default_screen()), "t1", 0);
    pollster::block_on(async {
        assert!(
            !sentinel
                .authorize(&context, &tap(Some(point(1, 1)), None, Some("支付")))
                .await
        );
    });
    let records = audit.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].approved, Some(false));
}

#[test]
fn task_approval_covers_same_deeplink_host() {
    let f = fixture();
    f.gateway.answer(ApprovalAnswer {
        approved: true,
        scope: ApprovalScope::Task,
    });
    pollster::block_on(async {
        assert!(
            f.sentinel
                .authorize(
                    &ctx(None, Some(default_screen()), "t1", 0),
                    &deeplink("didi://a?x=1", None)
                )
                .await
        );
        assert!(
            f.sentinel
                .authorize(
                    &ctx(None, Some(default_screen()), "t1", 1),
                    &deeplink("didi://a?x=2", None)
                )
                .await
        );
        f.gateway.answer(ApprovalAnswer::once(false));
        assert!(
            !f.sentinel
                .authorize(
                    &ctx(None, Some(default_screen()), "t1", 2),
                    &deeplink("didi://b", None)
                )
                .await
        );
    });
    assert_eq!(f.gateway.request_count(), 2);
}
