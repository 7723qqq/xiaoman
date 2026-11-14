//! The authoring layer: render, validate, refuse secrets, write only the user root.

use std::fs;
use std::path::Path;

use tempfile::TempDir;
use xiaoman_api::{SkillAction, SkillAuthoring, SkillChannel, SkillFields};
use xiaoman_skills::{DirectorySkillAuthoring, SkillParser, render_skill_md};

fn authoring() -> (TempDir, TempDir, DirectorySkillAuthoring) {
    let bundled = TempDir::new().unwrap();
    let user = TempDir::new().unwrap();
    let authoring = DirectorySkillAuthoring::new(vec![bundled.path().to_path_buf()], user.path());
    (bundled, user, authoring)
}

fn write_folder(root: &Path, name: &str, content: &str, flow: Option<&str>) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), content).unwrap();
    if let Some(flow) = flow {
        fs::write(dir.join("flow.yaml"), flow).unwrap();
    }
}

fn gui_fields(name: &str) -> SkillFields {
    let mut fields = SkillFields::new(name, "打车回家", SkillChannel::Gui, "先读记忆，再打车。");
    fields.apps = vec!["com.autonavi.minimap".to_string()];
    fields.confirm_before = vec!["确认呼叫".to_string()];
    fields.memory_refs = vec!["places/家".to_string()];
    fields.deeplinks = vec![(
        "route".to_string(),
        "amapuri://route/plan/?dname={destination}".to_string(),
    )];
    fields
}

fn assert_no_temp_files(dir: &Path) {
    let names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().all(|name| !name.ends_with(".tmp")),
        "temp files left in {dir:?}: {names:?}"
    );
}

#[test]
fn rendered_skill_round_trips() {
    let fields = gui_fields("taxi-home");
    let rendered = render_skill_md(&fields);
    let (skill, problems) = SkillParser::parse_folder("taxi-home", &rendered, None, "test");
    assert!(problems.is_empty(), "{problems:?}");
    let skill = skill.expect("skill");
    assert_eq!(skill.name, fields.name);
    assert_eq!(skill.description, fields.description);
    assert_eq!(skill.channel, SkillChannel::Gui);
    assert_eq!(skill.apps, fields.apps);
    assert_eq!(skill.confirm_before, fields.confirm_before);
    assert_eq!(skill.memory_refs, fields.memory_refs);
    assert_eq!(skill.deeplinks, fields.deeplinks);
    assert_eq!(skill.instructions, fields.instructions);
}

#[test]
fn chat_skill_rejects_apps_and_deeplinks() {
    let (_bundled, _user, authoring) = authoring();
    for mutate in [
        |fields: &mut SkillFields| fields.apps.push("com.example".to_string()),
        |fields: &mut SkillFields| {
            fields
                .deeplinks
                .push(("search".to_string(), "example://go".to_string()));
        },
        |fields: &mut SkillFields| fields.confirm_before.push("确认".to_string()),
    ] {
        let mut fields = SkillFields::new(
            "daily-review",
            "每天复盘",
            SkillChannel::Chat,
            "先问三个问题，再写记忆。",
        );
        mutate(&mut fields);
        let error = authoring
            .prepare_write(SkillAction::Create, fields)
            .unwrap_err();
        assert!(error.contains("聊天工作流"), "{error}");
    }
}

#[test]
fn phone_number_is_rejected() {
    let (_bundled, _user, authoring) = authoring();
    let mut fields = SkillFields::new(
        "phone-test",
        "电话",
        SkillChannel::Chat,
        "有问题打 13812345678 找我。",
    );
    fields.apps.clear();
    let error = authoring
        .prepare_write(SkillAction::Create, fields)
        .unwrap_err();
    assert!(error.contains("手机号"), "{error}");
    assert!(!error.contains("13812345678"), "{error}");
}

#[test]
fn api_key_shape_is_rejected() {
    let (_bundled, _user, authoring) = authoring();
    let fields = SkillFields::new(
        "key-test",
        "密钥",
        SkillChannel::Chat,
        "调用时使用 sk-test12345678。",
    );
    let error = authoring
        .prepare_write(SkillAction::Create, fields)
        .unwrap_err();
    assert!(error.contains("API Key"), "{error}");
    assert!(!error.contains("sk-test12345678"), "{error}");
}

#[test]
fn assignment_is_rejected() {
    let (_bundled, _user, authoring) = authoring();
    let fields = SkillFields::new(
        "password-test",
        "赋值",
        SkillChannel::Chat,
        "password: hunter2 之后再继续。",
    );
    let error = authoring
        .prepare_write(SkillAction::Create, fields)
        .unwrap_err();
    assert!(error.contains("密码或密钥赋值"), "{error}");
    assert!(!error.contains("hunter2"), "{error}");
}

#[test]
fn builtin_cannot_be_deleted() {
    let (bundled, user, authoring) = authoring();
    let content = render_skill_md(&gui_fields("taxi-home"));
    write_folder(bundled.path(), "taxi-home", &content, None);

    let error = authoring.prepare_delete("taxi-home").unwrap_err();
    assert!(error.contains("内置技能"), "{error}");
    assert!(bundled.path().join("taxi-home/SKILL.md").is_file());
    assert!(!user.path().join("taxi-home").exists());
}

#[test]
fn create_writes_only_the_user_root() {
    let (bundled, user, authoring) = authoring();
    let bundled_content = render_skill_md(&gui_fields("taxi-home"));
    write_folder(bundled.path(), "taxi-home", &bundled_content, None);

    let draft = authoring
        .prepare_write(SkillAction::Create, gui_fields("night-taxi"))
        .expect("draft");
    authoring.commit(&draft).expect("commit");

    let saved = user.path().join("night-taxi/SKILL.md");
    assert!(saved.is_file());
    let (skill, problems) = SkillParser::parse_folder(
        "night-taxi",
        &fs::read_to_string(&saved).unwrap(),
        None,
        "test",
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(skill.unwrap().channel, SkillChannel::Gui);
    assert_eq!(
        fs::read_to_string(bundled.path().join("taxi-home/SKILL.md")).unwrap(),
        bundled_content
    );
    assert_no_temp_files(&user.path().join("night-taxi"));
}

#[test]
fn update_keeps_flow_bytes() {
    let (bundled, user, authoring) = authoring();
    let flow = "steps:\n  - wait: 1500\n";
    write_folder(
        user.path(),
        "taxi-home",
        &render_skill_md(&gui_fields("taxi-home")),
        Some(flow),
    );
    let before = fs::read(user.path().join("taxi-home/flow.yaml")).unwrap();

    let mut fields = gui_fields("taxi-home");
    fields.description = "打车回家（新版）".to_string();
    let draft = authoring
        .prepare_write(SkillAction::Update, fields)
        .expect("draft");
    authoring.commit(&draft).expect("commit");

    assert_eq!(
        fs::read(user.path().join("taxi-home/flow.yaml")).unwrap(),
        before
    );
    assert!(
        fs::read_to_string(user.path().join("taxi-home/SKILL.md"))
            .unwrap()
            .contains("新版")
    );
    assert!(!bundled.path().join("taxi-home").exists());
}

#[test]
fn update_with_broken_flow_is_rejected() {
    let (_bundled, user, authoring) = authoring();
    let flow = "steps:\n  - launch: com.other.app\n";
    write_folder(
        user.path(),
        "taxi-home",
        &render_skill_md(&gui_fields("taxi-home")),
        Some(flow),
    );
    let before = fs::read_to_string(user.path().join("taxi-home/SKILL.md")).unwrap();
    let mut fields = gui_fields("taxi-home");
    fields.description = "改坏流程".to_string();

    let error = authoring
        .prepare_write(SkillAction::Update, fields)
        .unwrap_err();
    assert!(error.contains("技能校验失败"), "{error}");
    assert_eq!(
        fs::read_to_string(user.path().join("taxi-home/SKILL.md")).unwrap(),
        before
    );
}

#[test]
fn read_returns_the_user_copy_and_the_bundled_flag() {
    let (bundled, user, authoring) = authoring();
    write_folder(
        bundled.path(),
        "taxi-home",
        &render_skill_md(&gui_fields("taxi-home")),
        None,
    );
    write_folder(
        user.path(),
        "taxi-home",
        "---\nname: taxi-home\ndescription: 用户版本\nchannel: gui\n---\n\n用户正文\n",
        None,
    );
    let read = authoring.read("taxi-home").expect("read");
    assert!(read.bundled);
    assert!(read.content.contains("用户正文"));
}

#[test]
fn create_refuses_an_existing_user_skill_and_update_refuses_a_missing_one() {
    let (_bundled, user, authoring) = authoring();
    write_folder(
        user.path(),
        "taxi-home",
        &render_skill_md(&gui_fields("taxi-home")),
        None,
    );
    let error = authoring
        .prepare_write(SkillAction::Create, gui_fields("taxi-home"))
        .unwrap_err();
    assert!(error.contains("已经存在"), "{error}");

    let error = authoring
        .prepare_write(SkillAction::Update, gui_fields("missing-skill"))
        .unwrap_err();
    assert!(error.contains("不存在"), "{error}");
}
