use std::path::{Path, PathBuf};

use xiaoman_cli::lint::{SkillInfo, lint_flow, lint_skill};

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

fn fixture() -> PathBuf {
    repo("crates/xiaoman-cli/tests/fixtures/skills/ride-home")
}

fn msg(file: &str, line: usize, message: &str) -> (String, usize, String) {
    (file.to_string(), line, message.to_string())
}

fn messages(info: &SkillInfo) -> Vec<(String, usize, String)> {
    info.problems
        .iter()
        .map(|problem| {
            let file = Path::new(&problem.path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (file, problem.line, problem.message.clone())
        })
        .collect()
}

struct LintCase {
    tmp: tempfile::TempDir,
}

impl LintCase {
    fn new() -> Self {
        Self {
            tmp: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn skill(&self, folder: &str, skill_md: &str, flow: Option<&str>) -> PathBuf {
        let directory = self.tmp.path().join(folder);
        std::fs::create_dir_all(&directory).expect("mkdir");
        std::fs::write(directory.join("SKILL.md"), skill_md).expect("write SKILL.md");
        if let Some(flow) = flow {
            std::fs::write(directory.join("flow.yaml"), flow).expect("write flow.yaml");
        }
        directory
    }
}

#[test]
fn fixture_is_clean() {
    let info = lint_skill(&fixture());
    assert_eq!(info.problems, Vec::new());
    assert_eq!(info.name.as_deref(), Some("ride-home"));
}

#[test]
fn problem_format() {
    let case = LintCase::new();
    let directory = case.skill("x", "---\nname: x\n---\n", None);
    let info = lint_skill(&directory);
    assert_eq!(
        info.problems
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec![format!(
            "{}:1: missing required key 'description'",
            directory.join("SKILL.md").display()
        )]
    );
}

#[test]
fn missing_skill_md() {
    let case = LintCase::new();
    let directory = case.tmp.path().join("empty");
    std::fs::create_dir_all(&directory).expect("mkdir");
    assert_eq!(
        lint_skill(&directory)
            .problems
            .iter()
            .map(|problem| problem.message.clone())
            .collect::<Vec<_>>(),
        vec!["missing SKILL.md".to_string()]
    );
}

#[test]
fn missing_frontmatter() {
    let case = LintCase::new();
    let info = lint_skill(&case.skill("x", "# no frontmatter\n", None));
    assert!(info.problems[0].message.contains("missing frontmatter"));
}

#[test]
fn invalid_yaml_line() {
    let case = LintCase::new();
    let info = lint_skill(&case.skill("x", "---\nname: x\napps: [a\n---\n", None));
    assert_eq!(info.problems.len(), 1);
    assert_eq!(info.problems[0].line, 3);
    assert_eq!("invalid YAML", info.problems[0].message);
}

#[test]
fn name_rules() {
    let case = LintCase::new();
    let info = lint_skill(&case.skill("Taxi", "---\nname: Taxi\ndescription: d\n---\n", None));
    assert_eq!(
        messages(&info),
        vec![msg("SKILL.md", 2, "name 'Taxi' must match [a-z0-9-]+")]
    );

    let info = lint_skill(&case.skill("folder", "---\ndescription: d\nname: other\n---\n", None));
    assert_eq!(
        messages(&info),
        vec![msg(
            "SKILL.md",
            3,
            "name 'other' does not match folder name 'folder'"
        )]
    );
    assert_eq!(info.name.as_deref(), Some("other"));

    let info = lint_skill(&case.skill("n", "---\nname:\ndescription: d\n---\n", None));
    assert_eq!(
        messages(&info),
        vec![msg("SKILL.md", 2, "'name' must be a string")]
    );
}

#[test]
fn description_rules() {
    let case = LintCase::new();
    let info = lint_skill(&case.skill("a", "---\nname: a\ndescription: \"\"\n---\n", None));
    assert_eq!(
        messages(&info),
        vec![msg("SKILL.md", 3, "'description' must not be empty")]
    );

    let info = lint_skill(&case.skill(
        "b",
        "---\nname: b\ndescription: |\n  one\n  two\n---\n",
        None,
    ));
    assert_eq!(
        messages(&info),
        vec![msg("SKILL.md", 3, "'description' must be a single line")]
    );

    let info = lint_skill(&case.skill("c", "---\nname: c\ndescription: [x]\n---\n", None));
    assert_eq!(
        messages(&info),
        vec![msg("SKILL.md", 3, "'description' must be a string")]
    );
}

#[test]
fn unknown_and_duplicate_keys() {
    let case = LintCase::new();
    let info = lint_skill(&case.skill(
        "a",
        "---\nname: a\ndescription: d\nversion: 2\nname: a\n---\n",
        None,
    ));
    let found = messages(&info);
    assert_eq!(found[0].2, "duplicate key 'name'");

    let info =
        lint_skill(&case.skill("b", "---\nname: b\ndescription: d\nversion: 2\n---\n", None));
    let found = messages(&info);
    assert_eq!(
        (found[0].1, found[0].2.as_str()),
        (
            4,
            "unknown key 'version'; expected one of name, description, apps, channel, confirm_before, \
             memory, deeplinks"
        )
    );
}

#[test]
fn channel() {
    let case = LintCase::new();
    let info = lint_skill(&case.skill(
        "a",
        "---\nname: a\ndescription: d\nchannel: GUI\n---\n",
        None,
    ));
    assert_eq!(info.problems, Vec::new());

    let info = lint_skill(&case.skill(
        "b",
        "---\nname: b\ndescription: d\nchannel: web\n---\n",
        None,
    ));
    assert_eq!(
        messages(&info),
        vec![msg(
            "SKILL.md",
            4,
            "unknown channel 'web'; expected one of api, cli, deeplink, gui, deeplink-then-gui, mixed, chat"
        )]
    );
}

#[test]
fn chat_allows_memory() {
    let case = LintCase::new();
    let md = "---\nname: a\ndescription: 排今日开工卡\nchannel: CHAT\nmemory: [偏好/工作]\n---\n安排计划";
    assert_eq!(lint_skill(&case.skill("a", md, None)).problems, Vec::new());
}

#[test]
fn chat_rejects_phone_fields_even_empty() {
    let case = LintCase::new();
    for (number, field) in ["apps: []", "deeplinks: {}", "confirm_before:"]
        .iter()
        .enumerate()
    {
        let name = format!("chat-{number}");
        let md = format!("---\nname: {name}\ndescription: d\nchannel: chat\n{field}\n---\n");
        let info = lint_skill(&case.skill(&name, &md, None));
        let key = field.split(':').next().unwrap();
        assert_eq!(
            messages(&info),
            vec![msg(
                "SKILL.md",
                5,
                &format!("聊天工作流不操作手机，不能设置 {key}")
            )]
        );
    }
}

#[test]
fn chat_rejects_flow_even_empty() {
    let case = LintCase::new();
    let md = "---\nname: a\ndescription: d\nchannel: chat\n---\n";
    assert_eq!(
        messages(&lint_skill(&case.skill("a", md, Some("")))),
        vec![msg(
            "flow.yaml",
            1,
            "聊天工作流不操作手机，不能设置 flow.yaml"
        )]
    );
}

#[test]
fn string_lists() {
    let case = LintCase::new();
    let ok = "---\nname: a\ndescription: d\napps: com.x.y\nmemory: 地址/家\nconfirm_before:\n---\n";
    assert_eq!(lint_skill(&case.skill("a", ok, None)).problems, Vec::new());

    let bad = "---\nname: b\ndescription: d\nconfirm_before: [x, [y]]\nmemory: {a: b}\napps: [com.x.y, chrome]\n---\n";
    assert_eq!(
        messages(&lint_skill(&case.skill("b", bad, None))),
        vec![
            msg("SKILL.md", 6, "apps: 'chrome' is not a package name"),
            msg("SKILL.md", 4, "'confirm_before' items must be strings"),
            msg("SKILL.md", 5, "'memory' must be a list of strings"),
        ]
    );

    let empty = "---\nname: c\ndescription: d\nmemory: [\"\", x]\n---\n";
    assert_eq!(
        messages(&lint_skill(&case.skill("c", empty, None))),
        vec![msg("SKILL.md", 4, "'memory' items must not be empty")]
    );
}

#[test]
fn deeplinks() {
    let case = LintCase::new();
    let md = "---\nname: a\ndescription: d\ndeeplinks:\n  ok: \"x://y?{q}\"\n  bad: http//x\n  sp: \"a:// b\"\n---\n";
    assert_eq!(
        messages(&lint_skill(&case.skill("a", md, None))),
        vec![
            msg(
                "SKILL.md",
                6,
                "deeplinks: 'bad' = 'http//x' is not a scheme:// URI"
            ),
            msg(
                "SKILL.md",
                7,
                "deeplinks: 'sp' = 'a:// b' is not a scheme:// URI"
            ),
        ]
    );

    let md = "---\nname: b\ndescription: d\ndeeplinks: [x://y]\n---\n";
    assert_eq!(
        messages(&lint_skill(&case.skill("b", md, None))),
        vec![msg(
            "SKILL.md",
            4,
            "'deeplinks' must be a mapping of name: \"scheme://...\""
        )]
    );
}

#[test]
fn flow_skipped_when_skill_md_invalid() {
    let case = LintCase::new();
    let info = lint_skill(&case.skill("a", "---\nname: a\n---\n", Some("steps: []\n")));
    let files: Vec<String> = info
        .problems
        .iter()
        .map(|problem| {
            Path::new(&problem.path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(files, vec!["SKILL.md".to_string()]);
}

fn flow_with_apps(text: &str, apps: &[&str]) -> Vec<(usize, String)> {
    let deeplinks = vec![("search".to_string(), "amapuri://x".to_string())];
    let apps: Vec<String> = apps.iter().map(|app| app.to_string()).collect();
    lint_flow("flow.yaml", text, &deeplinks, &apps)
        .into_iter()
        .map(|problem| (problem.line, problem.message))
        .collect()
}

fn flow(text: &str) -> Vec<(usize, String)> {
    flow_with_apps(text, &["com.autonavi.minimap"])
}

#[test]
fn flow_good() {
    assert_eq!(
        flow("steps:\n  - deeplink: search\n  - wait: '0'\n  - type: {target: a, text: ''}\n"),
        vec![]
    );
}

#[test]
fn flow_structure() {
    assert_eq!(
        flow(""),
        vec![(1, "flow.yaml is empty; expected 'steps:'".to_string())]
    );
    assert_eq!(
        flow("- tap: x"),
        vec![(1, "flow.yaml must be a mapping with 'steps:'".to_string())]
    );
    assert_eq!(
        flow("name: x\n"),
        vec![
            (1, "unknown key 'name'; only 'steps' is allowed".to_string()),
            (1, "missing 'steps:' list".to_string()),
        ]
    );
    assert_eq!(
        flow("steps: x\n"),
        vec![(1, "'steps' must be a list".to_string())]
    );
    assert_eq!(
        flow("steps: []\n"),
        vec![(1, "'steps' is empty".to_string())]
    );
}

#[test]
fn flow_step_shapes() {
    let text = concat!(
        "steps:\n",
        "  - tap: a\n    wait: 1\n",
        "  - click: x\n",
        "  - tap:\n",
        "  - ai: [x]\n",
        "  - launch: chrome\n",
        "  - launch: com.other.app\n",
        "  - deeplink: home\n",
        "  - wait: 600001\n",
        "  - wait: soon\n",
        "  - type: { target: x, text: {destination} }\n",
        "  - type: { target: x, text: y, submit: true }\n",
        "  - type: x\n",
        "  - just text\n",
    );
    let got = flow(text);
    assert_eq!(
        got.iter().map(|(line, _)| *line).collect::<Vec<_>>(),
        vec![2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
    );
    let messages: std::collections::HashMap<usize, String> = got.into_iter().collect();
    assert!(messages[&2].starts_with("step 1: expected one 'kind: value' pair"));
    assert!(messages[&4].starts_with("step 2 (click): unknown step kind"));
    assert_eq!(messages[&5], "step 3 (tap): expected a target description");
    assert_eq!(
        messages[&7],
        "step 5 (launch): 'chrome' is not a package name"
    );
    assert_eq!(
        messages[&8],
        "step 6 (launch): 'com.other.app' is not in the skill's apps"
    );
    assert_eq!(
        messages[&9],
        "step 7 (deeplink): 'home' is neither a key of the skill's deeplinks nor a scheme:// URI"
    );
    assert_eq!(
        messages[&10],
        "step 8 (wait): expected milliseconds as an integer 0..600000"
    );
    assert!(messages[&12].contains("quote values that start with '{'"));
    assert_eq!(
        messages[&13],
        "step 11 (type): expected { target: ..., text: ... }"
    );
}

#[test]
fn flow_launch_without_apps_and_literal_uri() {
    assert_eq!(
        flow_with_apps(
            "steps:\n  - launch: com.any.app\n  - deeplink: weixin://dl/scan\n",
            &[]
        ),
        vec![]
    );
}

#[test]
fn examples() {
    let root = repo("skills/examples");
    if !root.is_dir() {
        return;
    }
    for entry in std::fs::read_dir(&root).expect("read examples") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() && !name.starts_with('.') {
            let info = lint_skill(&entry.path());
            assert!(info.problems.is_empty(), "{name}: {:?}", info.problems);
        }
    }
}
