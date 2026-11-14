//! Renders, validates and commits `SKILL.md` drafts. Only [`DirectorySkillAuthoring::commit`]
//! writes to disk, and only below the user root.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use regex::Regex;
use xiaoman_api::{
    SkillAction, SkillAuthoring, SkillChannel, SkillDraft, SkillFields, SkillReadResult,
};

use crate::skill_parser::SkillParser;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

static PHONE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"1[3-9][0-9]{9}").expect("phone regex"));
static SK_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"sk-[A-Za-z0-9_-]{8,}").expect("sk key regex"));
static XM_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"xm-[A-Za-z0-9_-]{8,}").expect("xm key regex"));
static GITHUB_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"gh[pousr]_[A-Za-z0-9]{20,}").expect("github key regex"));
static AWS_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"AKIA[0-9A-Z]{16}").expect("aws key regex"));
static PRIVATE_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-----BEGIN[^-]*PRIVATE KEY-----").expect("private key regex"));
static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)bearer\s+[A-Za-z0-9._~+/=-]{8,}").expect("bearer regex"));
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(password|passwd|secret|api[_-]?key|access[_-]?token|token|密码|密钥)\s*[:：=]\s*\S+",
    )
    .expect("assignment regex")
});

/// A skill folder tree: bundled roots are read-only, `user_root` is the only writable place.
pub struct DirectorySkillAuthoring {
    bundled_roots: Vec<PathBuf>,
    user_root: PathBuf,
}

impl DirectorySkillAuthoring {
    pub fn new(bundled_roots: Vec<PathBuf>, user_root: impl Into<PathBuf>) -> Self {
        Self {
            bundled_roots,
            user_root: user_root.into(),
        }
    }

    pub fn user_root(&self) -> &Path {
        &self.user_root
    }

    fn user_skill(&self, name: &str) -> PathBuf {
        self.user_root.join(name).join("SKILL.md")
    }

    fn user_dir(&self, name: &str) -> PathBuf {
        self.user_root.join(name)
    }

    fn bundled_skill(&self, name: &str) -> Option<PathBuf> {
        self.bundled_roots
            .iter()
            .map(|root| root.join(name).join("SKILL.md"))
            .find(|path| path.is_file())
    }

    fn bundled_flow(&self, name: &str) -> Option<PathBuf> {
        self.bundled_roots
            .iter()
            .map(|root| root.join(name).join("flow.yaml"))
            .find(|path| path.is_file())
    }

    fn flow_for(&self, name: &str, user_exists: bool) -> Option<String> {
        let path = if user_exists {
            Some(self.user_dir(name).join("flow.yaml"))
        } else {
            self.bundled_flow(name)
        };
        path.filter(|path| path.is_file())
            .and_then(|path| fs::read_to_string(path).ok())
    }
}

impl SkillAuthoring for DirectorySkillAuthoring {
    fn read(&self, name: &str) -> Option<SkillReadResult> {
        if !valid_name(name) {
            return None;
        }
        let bundled = self.bundled_skill(name).is_some();
        let content = fs::read_to_string(self.user_skill(name)).ok().or_else(|| {
            self.bundled_skill(name)
                .and_then(|path| fs::read_to_string(path).ok())
        })?;
        Some(SkillReadResult {
            name: name.to_string(),
            content,
            bundled,
        })
    }

    fn prepare_write(
        &self,
        action: SkillAction,
        mut fields: SkillFields,
    ) -> Result<SkillDraft, String> {
        if !matches!(action, SkillAction::Create | SkillAction::Update) {
            return Err("prepare_write 只接受 create / update".to_string());
        }
        normalize(&mut fields);
        validate_fields(&fields)?;

        let name = fields.name.clone();
        let user_exists = self.user_skill(&name).is_file();
        let bundled_exists = self.bundled_skill(&name).is_some();
        match action {
            SkillAction::Create if user_exists => {
                return Err(format!("技能 {name} 已经存在，请用 update 修改"));
            }
            SkillAction::Update if !user_exists && !bundled_exists => {
                return Err(format!("技能 {name} 不存在，请用 create 新建"));
            }
            _ => {}
        }

        let flow = self.flow_for(&name, user_exists);
        let content = render_skill_md(&fields);
        validate_rendered(&name, &content, flow.as_deref())?;
        scan(&content)?;
        let previous = fs::read_to_string(self.user_skill(&name)).ok().or_else(|| {
            self.bundled_skill(&name)
                .and_then(|path| fs::read_to_string(path).ok())
        });

        Ok(SkillDraft {
            action,
            name,
            content,
            previous,
            shadows_bundled: bundled_exists,
        })
    }

    fn prepare_delete(&self, name: &str) -> Result<SkillDraft, String> {
        if !valid_name(name) {
            return Err(format!("技能名 {name} 不合法，必须匹配 [a-z0-9-]+"));
        }
        if self.bundled_skill(name).is_some() {
            return Err(format!("内置技能 {name} 不能删除"));
        }
        let previous = fs::read_to_string(self.user_skill(name))
            .map_err(|_| format!("没有用户技能 {name}"))?;
        Ok(SkillDraft {
            action: SkillAction::Delete,
            name: name.to_string(),
            content: String::new(),
            previous: Some(previous),
            shadows_bundled: false,
        })
    }

    fn commit(&self, draft: &SkillDraft) -> Result<(), String> {
        if !valid_name(&draft.name) {
            return Err(format!("技能名 {} 不合法", draft.name));
        }
        if draft.action == SkillAction::Delete {
            if self.bundled_skill(&draft.name).is_some() {
                return Err(format!("内置技能 {} 不能删除", draft.name));
            }
            let dir = self.user_dir(&draft.name);
            if dir.is_dir() {
                fs::remove_dir_all(&dir)
                    .map_err(|error| format!("删除 {} 失败：{error}", dir.display()))?;
            }
            return Ok(());
        }

        scan(&draft.content)?;
        let user_exists = self.user_skill(&draft.name).is_file();
        let flow = self.flow_for(&draft.name, user_exists);
        validate_rendered(&draft.name, &draft.content, flow.as_deref())?;

        let dir = self.user_dir(&draft.name);
        fs::create_dir_all(&dir)
            .map_err(|error| format!("创建 {} 失败：{error}", dir.display()))?;
        // A user edit keeps its own flow.yaml; a new user copy of a bundled skill inherits the
        // bundled flow once, so the behaviour does not silently change.
        let flow_path = dir.join("flow.yaml");
        if !flow_path.exists() {
            if let Some(bundled) = self.bundled_flow(&draft.name) {
                fs::copy(&bundled, &flow_path)
                    .map_err(|error| format!("复制 {} 失败：{error}", bundled.display()))?;
            }
        }
        atomic_write(&dir.join("SKILL.md"), &draft.content)
    }
}

/// Renders the parser's seven frontmatter keys in its canonical order.
pub fn render_skill_md(fields: &SkillFields) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("name: {}\n", yaml_string(&fields.name)));
    out.push_str(&format!(
        "description: {}\n",
        yaml_string(&fields.description)
    ));
    if !fields.apps.is_empty() {
        out.push_str("apps:\n");
        for app in &fields.apps {
            out.push_str(&format!("  - {}\n", yaml_string(app)));
        }
    }
    out.push_str(&format!("channel: {}\n", fields.channel.wire()));
    if !fields.confirm_before.is_empty() {
        out.push_str("confirm_before:\n");
        for word in &fields.confirm_before {
            out.push_str(&format!("  - {}\n", yaml_string(word)));
        }
    }
    if !fields.memory_refs.is_empty() {
        out.push_str("memory:\n");
        for reference in &fields.memory_refs {
            out.push_str(&format!("  - {}\n", yaml_string(reference)));
        }
    }
    if !fields.deeplinks.is_empty() {
        out.push_str("deeplinks:\n");
        for (key, value) in &fields.deeplinks {
            out.push_str(&format!("  {}: {}\n", yaml_string(key), yaml_string(value)));
        }
    }
    out.push_str("---\n\n");
    out.push_str(fields.instructions.trim_end());
    out.push('\n');
    out
}

fn normalize(fields: &mut SkillFields) {
    fields.name = fields.name.trim().to_string();
    fields.description = fields.description.trim().to_string();
    fields.instructions = fields.instructions.trim().to_string();
    for app in &mut fields.apps {
        *app = app.trim().to_string();
    }
    for word in &mut fields.confirm_before {
        *word = word.trim().to_string();
    }
    for reference in &mut fields.memory_refs {
        *reference = reference.trim().to_string();
    }
    for (key, value) in &mut fields.deeplinks {
        *key = key.trim().to_string();
        *value = value.trim().to_string();
    }
}

fn validate_fields(fields: &SkillFields) -> Result<(), String> {
    if !valid_name(&fields.name) {
        return Err(format!(
            "技能名 {} 不合法，必须匹配 [a-z0-9-]+ 且不超过 64 个字符",
            fields.name
        ));
    }
    if fields.description.is_empty() {
        return Err("description 不能为空".to_string());
    }
    if fields.description.contains('\n') {
        return Err("description 必须是单行".to_string());
    }
    if fields.description.chars().count() > 200 {
        return Err("description 不能超过 200 个字符".to_string());
    }
    if fields.instructions.trim().is_empty() {
        return Err("instructions 不能为空".to_string());
    }
    if fields.instructions.chars().count() > 20_000 {
        return Err("instructions 不能超过 20000 个字符".to_string());
    }
    if fields.channel == SkillChannel::Chat {
        for (key, present) in [
            ("apps", !fields.apps.is_empty()),
            ("confirm_before", !fields.confirm_before.is_empty()),
            ("deeplinks", !fields.deeplinks.is_empty()),
        ] {
            if present {
                return Err(format!("聊天工作流不操作手机，不能设置 {key}"));
            }
        }
    }
    Ok(())
}

fn validate_rendered(name: &str, content: &str, flow: Option<&str>) -> Result<(), String> {
    let (skill, problems) = SkillParser::parse_folder(name, content, flow, "<draft>");
    if !problems.is_empty() {
        let messages: Vec<String> = problems
            .iter()
            .map(|problem| match problem.line {
                Some(line) => format!("{}:{}: {}", problem.source, line, problem.message),
                None => format!("{}: {}", problem.source, problem.message),
            })
            .collect();
        return Err(format!("技能校验失败：{}", messages.join("；")));
    }
    if skill.is_none() {
        return Err("技能校验失败".to_string());
    }
    Ok(())
}

fn scan(content: &str) -> Result<(), String> {
    for (name, hit) in [
        ("手机号", phone_hit(content)),
        (
            "API Key",
            SK_KEY.is_match(content) || XM_KEY.is_match(content),
        ),
        (
            "访问令牌",
            GITHUB_KEY.is_match(content) || AWS_KEY.is_match(content) || BEARER.is_match(content),
        ),
        ("私钥", PRIVATE_KEY.is_match(content)),
        ("密码或密钥赋值", ASSIGNMENT.is_match(content)),
    ] {
        if hit {
            return Err(format!(
                "技能内容包含疑似{name}；请删除具体值后重试（说明文字可以保留）"
            ));
        }
    }
    Ok(())
}

fn phone_hit(content: &str) -> bool {
    PHONE.find_iter(content).any(|hit| {
        let before = content[..hit.start()].chars().next_back();
        let after = content[hit.end()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_digit()) && !after.is_some_and(|c| c.is_ascii_digit())
    })
}

fn valid_name(name: &str) -> bool {
    name.chars().count() <= 64 && SkillParser::name_pattern().is_match(name)
}

fn yaml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn atomic_write(file: &Path, text: &str) -> Result<(), String> {
    let dir = file
        .parent()
        .ok_or_else(|| format!("{} 没有父目录", file.display()))?;
    let unique = format!(
        ".SKILL.md.{}.{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let temp = dir.join(unique);
    let result = (|| -> std::io::Result<()> {
        let mut handle = fs::File::create(&temp)?;
        handle.write_all(text.as_bytes())?;
        handle.sync_all()?;
        drop(handle);
        fs::rename(&temp, file)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|error| format!("写入 {} 失败：{error}", file.display()))
}
