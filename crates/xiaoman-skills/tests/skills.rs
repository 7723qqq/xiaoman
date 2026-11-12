use std::path::{Path, PathBuf};
use std::sync::Arc;

use xiaoman_api::{FlowStep, Skill, SkillChannel, SkillProblem, SkillRepository};
use xiaoman_skills::{DirectorySkillRepository, FlowParser, SkillParser};

const VALID: &str = "---\nname: taxi-home\ndescription: 打车回家\napps: [com.autonavi.minimap]\nchannel: deeplink-then-gui\nconfirm_before: [确认呼叫, 支付]\nmemory: [places/家, 偏好/车型]\ndeeplinks:\n  search: \"amapuri://route/plan/?dname={destination}\"\n---\n# 打车回家\n\n先打开高德。\n";

fn parse(md: &str) -> (Option<Skill>, Vec<SkillProblem>) {
    SkillParser::parse(md, "/skills/x")
}

fn frontmatter(lines: &[&str]) -> String {
    format!("---\n{}\n---\nbody", lines.join("\n"))
}

fn problem(md: &str, fragment: &str) -> SkillProblem {
    let (skill, problems) = parse(md);
    assert!(skill.is_none(), "skill should be rejected: {problems:?}");
    let matches: Vec<&SkillProblem> = problems
        .iter()
        .filter(|problem| problem.message.contains(fragment))
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected one '{fragment}' problem in {problems:?}"
    );
    assert_eq!(matches[0].source, "/skills/x/SKILL.md");
    matches[0].clone()
}

#[test]
fn parses_valid_skill() {
    let (skill, problems) = parse(VALID);
    assert!(problems.is_empty(), "{problems:?}");
    let skill = skill.expect("skill");
    assert_eq!(skill.name, "taxi-home");
    assert_eq!(skill.description, "打车回家");
    assert_eq!(skill.apps, vec!["com.autonavi.minimap"]);
    assert_eq!(skill.channel, SkillChannel::DeeplinkThenGui);
    assert_eq!(skill.confirm_before, vec!["确认呼叫", "支付"]);
    assert_eq!(skill.memory_refs, vec!["places/家", "偏好/车型"]);
    assert_eq!(
        skill.deeplinks,
        vec![(
            "search".to_string(),
            "amapuri://route/plan/?dname={destination}".to_string()
        )]
    );
    assert_eq!(skill.instructions, "# 打车回家\n\n先打开高德。");
    assert_eq!(skill.source, "/skills/x");
    assert!(skill.flow.is_none());
}

#[test]
fn applies_defaults_and_accepts_single_strings() {
    let md = frontmatter(&[
        "name: a",
        "description: d",
        "confirm_before: 下单",
        "memory: places/家",
    ]);
    let (skill, problems) = parse(&md);
    assert!(problems.is_empty(), "{problems:?}");
    let skill = skill.expect("skill");
    assert_eq!(skill.channel, SkillChannel::Gui);
    assert!(skill.apps.is_empty());
    assert_eq!(skill.confirm_before, vec!["下单"]);
    assert_eq!(skill.memory_refs, vec!["places/家"]);
}

#[test]
fn accepts_bom() {
    assert!(parse(&format!("\u{feff}{VALID}")).0.is_some());
}

#[test]
fn missing_frontmatter() {
    assert_eq!(
        problem("# just markdown", "missing frontmatter").line,
        Some(1)
    );
    problem("---\nname: a\ndescription: d\n", "missing frontmatter");
}

#[test]
fn invalid_yaml_reports_line() {
    let found = problem(
        &frontmatter(&["name: a", "description: [unclosed"]),
        "invalid YAML",
    );
    assert!(
        found.line.is_some_and(|line| line >= 3),
        "line {:?}",
        found.line
    );
}

#[test]
fn name_required() {
    problem(
        &frontmatter(&["description: d"]),
        "missing required key 'name'",
    );
}

#[test]
fn name_format_with_line() {
    let found = problem(
        &frontmatter(&["description: d", "name: Taxi_Home"]),
        "must match [a-z0-9-]+",
    );
    assert_eq!(found.line, Some(3));
}

#[test]
fn description_required() {
    problem(
        &frontmatter(&["name: a"]),
        "missing required key 'description'",
    );
}

#[test]
fn description_single_line() {
    let found = problem(
        &frontmatter(&["name: a", "description: |", "  第一行", "  第二行"]),
        "single line",
    );
    assert_eq!(found.line, Some(3));
}

#[test]
fn apps_must_be_package_names() {
    let found = problem(
        &frontmatter(&["name: a", "description: d", "apps: [com.ok.app, 高德]"]),
        "'高德' is not a package name",
    );
    assert_eq!(found.line, Some(4));
    problem(
        &frontmatter(&["name: a", "description: d", "apps: {a: b}"]),
        "must be a list of strings",
    );
}

#[test]
fn unknown_channel() {
    problem(
        &frontmatter(&["name: a", "description: d", "channel: telepathy"]),
        "unknown channel 'telepathy'",
    );
}

#[test]
fn confirm_before_and_memory_must_be_strings() {
    problem(
        &frontmatter(&["name: a", "description: d", "confirm_before: [[支付]]"]),
        "items must be strings",
    );
    problem(
        &frontmatter(&["name: a", "description: d", "memory: {a: b}"]),
        "must be a list of strings",
    );
}

#[test]
fn deeplinks_must_be_uris() {
    let found = problem(
        &frontmatter(&[
            "name: a",
            "description: d",
            "deeplinks:",
            "  ok: \"a://b\"",
            "  bad: not-a-uri",
        ]),
        "'bad'",
    );
    assert_eq!(found.line, Some(6));
    problem(
        &frontmatter(&["name: a", "description: d", "deeplinks: [a://b]"]),
        "must be a mapping",
    );
}

#[test]
fn unknown_and_duplicate_keys() {
    problem(
        &frontmatter(&["name: a", "description: d", "confirm-before: [支付]"]),
        "unknown key 'confirm-before'",
    );
    problem(
        &frontmatter(&["name: a", "name: b", "description: d"]),
        "duplicate key 'name'",
    );
}

#[test]
fn folder_name_mismatch_is_reported_but_loads() {
    let (skill, problems) = SkillParser::parse_folder("other", VALID, None, "/skills/other");
    assert!(skill.is_some());
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].message.contains("does not match folder name"));
    assert_eq!(problems[0].line, Some(2));
}

#[test]
fn parse_folder_attaches_flow() {
    let flow = "steps:\n  - deeplink: search\n  - tap: 确认呼叫\n";
    let (skill, problems) =
        SkillParser::parse_folder("taxi-home", VALID, Some(flow), "/skills/taxi-home");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(skill.unwrap().flow.unwrap().steps.len(), 2);
}

#[test]
fn bad_flow_rejects_skill() {
    let (skill, problems) = SkillParser::parse_folder(
        "taxi-home",
        VALID,
        Some("steps:\n  - fly: away\n"),
        "/skills/taxi-home",
    );
    assert!(skill.is_none());
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].source, "/skills/taxi-home/flow.yaml");
}

#[test]
fn chat_workflow_allows_memory() {
    let md = frontmatter(&[
        "name: today-plan",
        "description: 排今日开工卡",
        "channel: CHAT",
        "memory: 偏好/工作",
    ]);
    let (skill, problems) = parse(&md);
    assert!(problems.is_empty(), "{problems:?}");
    let skill = skill.expect("skill");
    assert_eq!(skill.channel, SkillChannel::Chat);
    assert_eq!(skill.memory_refs, vec!["偏好/工作"]);
    assert_eq!(skill.instructions, "body");
}

#[test]
fn chat_workflow_rejects_phone_fields_even_when_empty() {
    for field in ["apps: []", "deeplinks: {}", "confirm_before:"] {
        let md = frontmatter(&["name: a", "description: d", "channel: chat", field]);
        let key = field.split(':').next().unwrap();
        let found = problem(&md, &format!("聊天工作流不操作手机，不能设置 {key}"));
        assert_eq!(found.line, Some(5), "{field}");
    }
}

#[test]
fn chat_workflow_rejects_flow_even_when_empty() {
    let md = frontmatter(&["name: a", "description: d", "channel: chat"]);
    let (skill, problems) = SkillParser::parse_folder("a", &md, Some(""), "/skills/a");
    assert!(skill.is_none());
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(
        problems[0].message,
        "聊天工作流不操作手机，不能设置 flow.yaml"
    );
    assert_eq!(problems[0].source, "/skills/a/flow.yaml");
}

fn deeplinks() -> Vec<(String, String)> {
    vec![(
        "search".to_string(),
        "amapuri://route/plan/?dname={destination}".to_string(),
    )]
}

fn parse_flow(text: &str, apps: &[&str]) -> (Option<xiaoman_api::Flow>, Vec<SkillProblem>) {
    let apps: Vec<String> = apps.iter().map(|app| (*app).to_string()).collect();
    FlowParser::parse(text, "flow.yaml", &deeplinks(), &apps)
}

fn single_problem(text: &str, fragment: &str, apps: &[&str]) -> Option<usize> {
    let (flow, problems) = parse_flow(text, apps);
    assert!(flow.is_none());
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].message.contains(fragment),
        "{}",
        problems[0].message
    );
    problems[0].line
}

#[test]
fn parses_all_step_kinds() {
    let text = "steps:\n  - launch: com.autonavi.minimap\n  - deeplink: search\n  - deeplink: \"amapuri://poi?name=家\"\n  - ai: 选择快车并读出预估价\n  - confirm: 确认呼叫快车？\n  - tap: 确认呼叫\n  - type: { target: 目的地, text: \"{destination}\" }\n  - wait: 1500\n  - assert: 行程已开始";
    let (flow, problems) = parse_flow(text, &["com.autonavi.minimap"]);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        flow.unwrap().steps,
        vec![
            FlowStep::Launch {
                package_name: "com.autonavi.minimap".into()
            },
            FlowStep::Deeplink {
                key_or_uri: "search".into()
            },
            FlowStep::Deeplink {
                key_or_uri: "amapuri://poi?name=家".into()
            },
            FlowStep::Ai {
                goal: "选择快车并读出预估价".into()
            },
            FlowStep::Confirm {
                question: "确认呼叫快车？".into()
            },
            FlowStep::Tap {
                target: "确认呼叫".into()
            },
            FlowStep::Type {
                target: "目的地".into(),
                text: "{destination}".into()
            },
            FlowStep::Wait { millis: 1500 },
            FlowStep::Assert {
                expectation: "行程已开始".into()
            },
        ]
    );
}

#[test]
fn unknown_step_kind_reports_index_and_line() {
    let line = single_problem(
        "steps:\n  - tap: a\n  - swipe: up\n",
        "step 2 (swipe): unknown step kind",
        &[],
    );
    assert_eq!(line, Some(3));
}

#[test]
fn wrong_shapes() {
    single_problem("steps:\n  - wait: soon\n", "step 1 (wait)", &[]);
    single_problem("steps:\n  - wait: -5\n", "step 1 (wait)", &[]);
    single_problem("steps:\n  - type: 目的地\n", "step 1 (type)", &[]);
    single_problem(
        "steps:\n  - type: { target: 目的地 }\n",
        "step 1 (type)",
        &[],
    );
    single_problem(
        "steps:\n  - type: { target: a, text: b, extra: c }\n",
        "step 1 (type)",
        &[],
    );
    single_problem("steps:\n  - tap:\n", "step 1 (tap)", &[]);
    single_problem(
        "steps:\n  - tap: a\n    wait: 5\n",
        "step 1: expected one",
        &[],
    );
    single_problem("steps:\n  - just text\n", "step 1: expected one", &[]);
    single_problem("steps:\n  - launch: 高德\n", "not a package name", &[]);
}

#[test]
fn unknown_deeplink_key() {
    let line = single_problem(
        "steps:\n  - deeplink: home\n",
        "'home' is neither a key",
        &[],
    );
    assert_eq!(line, Some(2));
}

#[test]
fn launch_outside_apps() {
    single_problem(
        "steps:\n  - launch: com.other.app\n",
        "not in the skill's apps",
        &["com.autonavi.minimap"],
    );
}

#[test]
fn bad_top_level() {
    single_problem("", "empty", &[]);
    single_problem("- tap: a\n", "must be a mapping", &[]);
    let (flow, problems) = parse_flow("step:\n  - tap: a\n", &[]);
    assert!(flow.is_none());
    let messages: Vec<String> = problems
        .iter()
        .map(|problem| {
            let head = problem.message.split(';').next().unwrap_or_default();
            head.split(" list").next().unwrap_or_default().to_string()
        })
        .collect();
    assert_eq!(messages, vec!["unknown key 'step'", "missing 'steps:'"]);
    single_problem("steps: []\n", "'steps' is empty", &[]);
    single_problem("steps: tap\n", "'steps' must be a list", &[]);
}

#[test]
fn reports_every_bad_step() {
    let (flow, problems) = parse_flow("steps:\n  - fly: a\n  - tap: ok\n  - wait: x\n", &[]);
    assert!(flow.is_none());
    assert_eq!(
        problems
            .iter()
            .map(|problem| problem.line)
            .collect::<Vec<_>>(),
        vec![Some(2), Some(4)]
    );
}

fn write_skill(root: &Path, folder: &str, name: &str, description: &str, flow: Option<&str>) {
    let dir = root.join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n说明 {folder}\n"),
    )
    .unwrap();
    if let Some(flow) = flow {
        std::fs::write(dir.join("flow.yaml"), flow).unwrap();
    }
}

fn examples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/examples")
}

#[tokio::test]
async fn loads_examples_without_problems() {
    let root = examples_root();
    assert!(root.is_dir(), "missing {}", root.display());
    let repo = DirectorySkillRepository::new(vec![root]);
    let result = repo.reload().await;
    assert!(result.problems.is_empty(), "{:?}", result.problems);
    assert_eq!(
        result
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "ai-news-brief",
            "daily-quiz",
            "health-alarms",
            "meituan-takeout-order",
            "open-settings-wifi",
            "set-alarm",
            "takeout-reorder",
            "taobao-compare",
            "taxi-home",
            "today-plan",
            "train-ticket-order",
        ]
    );
    let taxi = result
        .skills
        .iter()
        .find(|skill| skill.name == "taxi-home")
        .unwrap();
    assert_eq!(taxi.confirm_before, vec!["确认呼叫"]);
    assert_eq!(
        taxi.flow.as_ref().unwrap().steps.first(),
        Some(&FlowStep::Deeplink {
            key_or_uri: "route".into()
        })
    );
}

#[tokio::test]
async fn later_roots_override_earlier_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let bundled = tmp.path().join("bundled");
    let user = tmp.path().join("user");
    write_skill(&bundled, "taxi-home", "taxi-home", "内置", None);
    write_skill(
        &bundled,
        "open-settings-wifi",
        "open-settings-wifi",
        "描述",
        None,
    );
    write_skill(&user, "taxi-home", "taxi-home", "用户", None);

    let repo = DirectorySkillRepository::new(vec![bundled, user.clone()]);
    assert_eq!(repo.get("taxi-home").await.unwrap().description, "用户");
    assert_eq!(
        repo.list()
            .await
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["open-settings-wifi", "taxi-home"]
    );
    assert!(
        repo.get("taxi-home")
            .await
            .unwrap()
            .source
            .starts_with(&*user.to_string_lossy())
    );
}

#[tokio::test]
async fn broken_user_skill_keeps_bundled_one() {
    let tmp = tempfile::tempdir().unwrap();
    let bundled = tmp.path().join("bundled");
    let user = tmp.path().join("user");
    write_skill(&bundled, "taxi-home", "taxi-home", "内置", None);
    write_skill(
        &user,
        "taxi-home",
        "taxi-home",
        "描述",
        Some("steps:\n  - fly: away\n"),
    );

    let result = DirectorySkillRepository::new(vec![bundled, user])
        .reload()
        .await;
    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].description, "内置");
    assert_eq!(result.problems.len(), 1);
}

#[tokio::test]
async fn one_bad_skill_does_not_block_others() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "good", "good", "描述", None);
    write_skill(tmp.path(), "bad-channel", "bad-channel", "描述", None);
    std::fs::write(
        tmp.path().join("bad-channel/SKILL.md"),
        "---\nname: bad-channel\ndescription: d\nchannel: nope\n---\n",
    )
    .unwrap();
    std::fs::create_dir_all(tmp.path().join("no-skill-md")).unwrap();
    std::fs::create_dir_all(tmp.path().join(".hidden")).unwrap();
    std::fs::write(tmp.path().join("README.txt"), "not a skill").unwrap();

    let result = DirectorySkillRepository::new(vec![tmp.path().to_path_buf()])
        .reload()
        .await;
    assert_eq!(
        result
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["good"]
    );
    assert_eq!(result.problems.len(), 2, "{:?}", result.problems);
    assert!(
        result
            .problems
            .iter()
            .any(|problem| problem.message.contains("unknown channel") && problem.line == Some(4))
    );
    assert!(
        result
            .problems
            .iter()
            .any(|problem| problem.message == "missing SKILL.md")
    );
}

#[tokio::test]
async fn duplicate_names_in_one_root() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "taxi-home", "taxi-home", "描述", None);
    write_skill(tmp.path(), "taxi-copy", "taxi-home", "副本", None);

    let result = DirectorySkillRepository::new(vec![tmp.path().to_path_buf()])
        .reload()
        .await;
    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].description, "描述");
    assert!(
        result
            .problems
            .iter()
            .any(|problem| problem.message.contains("does not match folder name"))
    );
    assert!(result.problems.iter().any(|problem| {
        problem.message.contains("duplicate skill name 'taxi-home'")
            && problem.source.ends_with("taxi-copy")
    }));
}

#[tokio::test]
async fn missing_root_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let result = DirectorySkillRepository::new(vec![tmp.path().join("nope")])
        .reload()
        .await;
    assert!(result.skills.is_empty());
    assert!(result.problems.is_empty());
}

#[tokio::test]
async fn loads_lazily_and_reload_picks_up_changes() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "a", "a", "描述", None);
    let repo = Arc::new(DirectorySkillRepository::new(vec![
        tmp.path().to_path_buf(),
    ]));

    let mut handles = Vec::new();
    for _ in 0..5 {
        let repo = repo.clone();
        handles.push(tokio::spawn(async move { repo.list().await }));
    }
    for handle in handles {
        assert_eq!(
            handle
                .await
                .unwrap()
                .iter()
                .map(|skill| skill.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
    }

    write_skill(tmp.path(), "b", "b", "描述", None);
    assert!(repo.get("b").await.is_none(), "no rescan before reload");
    repo.reload().await;
    assert!(repo.get("b").await.is_some());
}
