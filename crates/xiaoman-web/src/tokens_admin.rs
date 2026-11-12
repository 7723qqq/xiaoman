//! `tokens ...`: manages the class-token file on the server itself (there is no admin network API).
//!
//! Run it as the service user (deploy installs `xiaoman-tokens`, which does that); the running server
//! picks up changes within about a second.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::claims::{LABEL, active_claims};
use crate::class_tokens::{
    ClassToken, ClassTokenCodec, ClassTokenStore, ForumClaim, PREFIX, StoreError, after_days,
    after_hours, display, offset,
};
use crate::memory::Clock;

pub const DEFAULT_FILE: &str = "/var/lib/xiaoman-web/class-tokens.json";
pub const DEFAULT_DAILY: i64 = 500_000;
pub const DEFAULT_TOTAL: i64 = 2_000_000;

pub fn usage() -> String {
    format!(
        r#"课堂令牌管理（只在服务器本机运行；服务运行中修改约 1 秒内生效）
用法：tokens <命令> [选项]
  generate N [--label 学员] [--start 1] [--days 3 | --hours H | --expires "2026-11-08 22:00"]
             [--daily {DEFAULT_DAILY}|none] [--total {DEFAULT_TOTAL}|none] [--csv] [--base-url URL] [--model 名称]
      生成 N 个令牌，标签为「学员 01」「学员 02」…（编号默认接着已有的）。明文只在这里显示一次；
      --csv 把 CSV（标签,令牌,过期时间,…）输出到标准输出，可重定向到文件发给学员。
  list                         列出全部令牌：状态、今日用量/每日上限、累计/总上限、请求数
  revoke <标签|令牌|前缀>...   作废指定令牌
  restore <标签|令牌|前缀>...  恢复误作废的令牌
  revoke-all                   全部作废（课后执行）
  limit <标签|令牌|前缀|--all> [--daily N|none] [--total N|none] [--days D | --hours H | --expires 时间]
      调整上限或过期时间
  claims                       列出自助领取记录：论坛账号 → 令牌前缀、状态、用量、领取时间
  reset-claim <论坛用户名|论坛用户ID>...
      允许该论坛账号重新领取，同时作废它原来的令牌（只想恢复原令牌用 restore 论坛:用户名）
通用选项：--file PATH（默认 $XIAOMAN_CLASS_TOKENS 或 {DEFAULT_FILE}），--timezone Asia/Shanghai
用量单位是模型 token（输入 + 输出），按方舟返回的 usage 记账；运行中的服务每 5 秒写回一次。"#
    )
}

/// Why a command stopped; the exit code follows the Kotlin admin command.
enum CliError {
    /// Bad arguments: the usage text follows the message.
    Usage(String),
    /// The command refused to act (nothing matched, nothing to reset).
    Message(String),
    Store(StoreError),
}

impl From<StoreError> for CliError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Rejected(message) | StoreError::Invalid(message) => Self::Message(message),
            io => Self::Store(io),
        }
    }
}

/// Remaining arguments, with the value options taken out as they are read.
struct Args {
    rest: Vec<String>,
}

impl Args {
    fn option(&mut self, name: &str) -> Result<Option<String>, CliError> {
        let Some(index) = self.rest.iter().position(|argument| argument == name) else {
            return Ok(None);
        };
        if index + 1 >= self.rest.len() {
            return Err(CliError::Usage(format!("{name} 需要一个值")));
        }
        let value = self.rest.remove(index + 1);
        self.rest.remove(index);
        Ok(Some(value))
    }

    fn flag(&mut self, name: &str) -> bool {
        match self.rest.iter().position(|argument| argument == name) {
            Some(index) => {
                self.rest.remove(index);
                true
            }
            None => false,
        }
    }

    fn take_first(&mut self) -> Option<String> {
        (!self.rest.is_empty()).then(|| self.rest.remove(0))
    }
}

/// The admin command; `out`, `err`, `env` and `clock` are injected so tests drive it like the server does.
pub fn run(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    env: &dyn Fn(&str) -> Option<String>,
    clock: Clock,
) -> i32 {
    let mut args = Args {
        rest: args.to_vec(),
    };
    let mut file: Option<PathBuf> = None;
    let result = execute(&mut args, &mut file, out, err, env, clock.clone());
    match result {
        Ok(()) => 0,
        Err(CliError::Usage(message)) => {
            let _ = writeln!(err, "错误：{message}\n\n{}", usage());
            2
        }
        Err(CliError::Message(message)) => {
            let _ = writeln!(err, "错误：{message}");
            2
        }
        Err(CliError::Store(error)) => {
            match &error {
                StoreError::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied => {
                    let path = file.as_deref().unwrap_or_else(|| Path::new(DEFAULT_FILE));
                    let _ = writeln!(
                        err,
                        "错误：没有权限读写令牌文件 {}。请用 sudo tokens …（以服务用户运行）",
                        path.display()
                    );
                }
                _ => {
                    let _ = writeln!(err, "错误：{error}");
                }
            }
            1
        }
    }
}

fn execute(
    args: &mut Args,
    file_slot: &mut Option<PathBuf>,
    out: &mut dyn Write,
    err: &mut dyn Write,
    env: &dyn Fn(&str) -> Option<String>,
    clock: Clock,
) -> Result<(), CliError> {
    if args.rest.is_empty() || matches!(args.rest[0].as_str(), "-h" | "--help" | "help") {
        let _ = writeln!(out, "{}", usage());
        return Ok(());
    }
    let file = match args.option("--file")? {
        Some(file) => PathBuf::from(file),
        None => env("XIAOMAN_CLASS_TOKENS")
            .filter(|value| !value.trim().is_empty())
            .map_or_else(|| PathBuf::from(DEFAULT_FILE), PathBuf::from),
    };
    *file_slot = Some(file.clone());
    let zone = match args.option("--timezone")? {
        Some(zone) => zone_offset(&zone)?,
        None => env("XIAOMAN_TZ")
            .filter(|value| !value.trim().is_empty())
            .map_or(Ok(480), |zone| zone_offset(&zone))?,
    };
    let store = ClassTokenStore::new(file.clone(), zone, clock.clone(), 0);
    let command = args.take_first().unwrap_or_default();
    match command.as_str() {
        "generate" => generate(args, &store, &file, zone, out, err, clock),
        "list" => list(args, &store, zone, out),
        "revoke" | "restore" => revoke(args, &store, &command == "revoke", err),
        "revoke-all" => {
            if !args.rest.is_empty() {
                return Err(CliError::Usage("revoke-all 不需要参数".to_string()));
            }
            let tokens = store.update(|tokens| {
                Ok(tokens
                    .into_iter()
                    .map(|token| ClassToken {
                        revoked: true,
                        ..token
                    })
                    .collect())
            })?;
            let _ = writeln!(err, "已作废全部 {} 个令牌。", tokens.len());
            Ok(())
        }
        "limit" => limit(args, &store, zone, err, clock),
        "claims" => claims(args, &store, zone, out),
        "reset-claim" => reset_claim(args, &store, err),
        other => Err(CliError::Usage(format!("未知命令 {other}"))),
    }
}

/// `generate N …`: plaintext is printed once; only hashes reach the file.
fn generate(
    args: &mut Args,
    store: &ClassTokenStore,
    file: &Path,
    zone: i32,
    out: &mut dyn Write,
    err: &mut dyn Write,
    clock: Clock,
) -> Result<(), CliError> {
    let label = args
        .option("--label")?
        .unwrap_or_else(|| "学员".to_string());
    let start = match args.option("--start")? {
        Some(value) => Some(
            value
                .parse::<i64>()
                .map_err(|_| CliError::Usage("--start 需要整数".to_string()))?,
        ),
        None => None,
    };
    let days = args.option("--days")?;
    let hours = args.option("--hours")?;
    let at = args.option("--expires")?;
    let now = clock();
    let expires = expiry(days.as_deref(), hours.as_deref(), at.as_deref(), zone, now)?
        .unwrap_or(now + 3 * 86_400_000);
    let daily = match args.option("--daily")? {
        Some(value) => limit_value("--daily", &value)?,
        None => Some(DEFAULT_DAILY),
    };
    let total = match args.option("--total")? {
        Some(value) => limit_value("--total", &value)?,
        None => Some(DEFAULT_TOTAL),
    };
    let csv = args.flag("--csv");
    let base_url = args.option("--base-url")?;
    let model = args.option("--model")?;
    let count = match args.rest.as_slice() {
        [count] => count
            .parse::<i64>()
            .ok()
            .filter(|count| (1..=500).contains(count))
            .ok_or_else(|| CliError::Usage("generate 需要数量 N（1-500）".to_string()))?,
        _ => return Err(CliError::Usage("generate 需要数量 N（1-500）".to_string())),
    };

    let mut created: Vec<(ClassToken, String)> = Vec::new();
    store.update(|tokens| {
        let pattern = format!("{label} ");
        let first = start.unwrap_or_else(|| {
            tokens
                .iter()
                .filter_map(|token| labelled_number(&token.label, &pattern))
                .max()
                .unwrap_or(0)
                + 1
        });
        let width = format!("{}", first + count - 1).len().max(2);
        let now = clock();
        let mut known: Vec<String> = tokens.iter().map(|token| token.hash.clone()).collect();
        for index in 0..count {
            let plain = loop {
                let candidate = ClassTokenCodec::generate();
                let hash = ClassTokenCodec::hash(&candidate);
                if !known.contains(&hash) {
                    known.push(hash);
                    break candidate;
                }
            };
            let token = ClassToken {
                hash: ClassTokenCodec::hash(&plain),
                prefix: plain.chars().take(6).collect(),
                label: format!("{label} {:0width$}", first + index, width = width),
                created_at: now,
                expires_at: Some(expires),
                revoked: false,
                daily_limit: daily,
                total_limit: total,
                used_total: 0,
                used_today: 0,
                usage_day: String::new(),
                requests: 0,
                last_used_at: None,
                claim: None,
            };
            created.push((token, plain));
        }
        let mut next = tokens;
        next.extend(created.iter().map(|(token, _)| token.clone()));
        Ok(next)
    })?;

    if csv {
        let _ = write!(out, "\u{feff}");
        let mut header = vec![
            "标签".to_string(),
            "令牌".to_string(),
            "过期时间".to_string(),
            "每日上限".to_string(),
            "总上限".to_string(),
        ];
        if base_url.is_some() {
            header.push("Base URL".to_string());
        }
        if model.is_some() {
            header.push("模型名".to_string());
        }
        let _ = writeln!(
            out,
            "{}",
            header
                .iter()
                .map(|value| csv_field(value))
                .collect::<Vec<_>>()
                .join(",")
        );
        for (token, plain) in &created {
            let mut row = vec![
                token.label.clone(),
                plain.clone(),
                time_text(token.expires_at, zone),
                amount(token.daily_limit),
                amount(token.total_limit),
            ];
            if let Some(base) = &base_url {
                row.push(base.clone());
            }
            if let Some(model) = &model {
                row.push(model.clone());
            }
            let _ = writeln!(
                out,
                "{}",
                row.iter()
                    .map(|value| csv_field(value))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    } else {
        for (token, plain) in &created {
            let _ = writeln!(out, "{}\t{}", token.label, plain);
        }
    }
    let _ = writeln!(
        err,
        "已生成 {} 个令牌（{}），过期时间 {}，每日上限 {}，总上限 {}。明文只显示这一次，请妥善保存。",
        created.len(),
        file.display(),
        time_text(Some(expires), zone),
        amount(daily),
        amount(total)
    );
    Ok(())
}

fn list(
    args: &mut Args,
    store: &ClassTokenStore,
    zone: i32,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    if !args.rest.is_empty() {
        return Err(CliError::Usage("list 不需要参数".to_string()));
    }
    let tokens = store.all();
    if tokens.is_empty() {
        let _ = writeln!(out, "（没有令牌：{}）", store.path().display());
        return Ok(());
    }
    let today = store.day();
    let _ = writeln!(
        out,
        "{}",
        [
            "标签",
            "前缀",
            "状态",
            "今日/每日上限",
            "累计/总上限",
            "请求数",
            "过期时间",
            "最近使用"
        ]
        .join("\t")
    );
    for token in &tokens {
        let _ = writeln!(
            out,
            "{}",
            [
                token.label.clone(),
                token.prefix.clone(),
                state_text(store, token),
                format!("{}/{}", token.used_on(&today), amount(token.daily_limit)),
                format!("{}/{}", token.used_total, amount(token.total_limit)),
                token.requests.to_string(),
                time_text(token.expires_at, zone),
                time_text(token.last_used_at, zone),
            ]
            .join("\t")
        );
    }
    let active = tokens
        .iter()
        .filter(|token| store.check(token).is_none())
        .count();
    let _ = writeln!(
        out,
        "共 {} 个，可用 {} 个，累计用量 {} token，今日 {} token",
        tokens.len(),
        active,
        tokens.iter().map(|token| token.used_total).sum::<i64>(),
        tokens
            .iter()
            .map(|token| token.used_on(&today))
            .sum::<i64>()
    );
    Ok(())
}

fn revoke(
    args: &mut Args,
    store: &ClassTokenStore,
    revoke: bool,
    err: &mut dyn Write,
) -> Result<(), CliError> {
    if args.rest.is_empty() {
        return Err(CliError::Usage(format!(
            "{} 需要标签、令牌或前缀",
            if revoke { "revoke" } else { "restore" }
        )));
    }
    let selectors = args.rest.clone();
    let written = store.update(|tokens| {
        let mut targets: Vec<String> = Vec::new();
        for selector in &selectors {
            targets.extend(
                select(&tokens, selector)?
                    .iter()
                    .map(|token| token.hash.clone()),
            );
        }
        Ok(tokens
            .into_iter()
            .map(|token| {
                if targets.contains(&token.hash) {
                    ClassToken {
                        revoked: revoke,
                        ..token
                    }
                } else {
                    token
                }
            })
            .collect())
    })?;
    let changed: Vec<ClassToken> = written
        .into_iter()
        .filter(|token| {
            selectors
                .iter()
                .any(|selector| select(std::slice::from_ref(token), selector).is_ok())
        })
        .collect();
    let _ = writeln!(
        err,
        "{}{}",
        if revoke {
            "已作废："
        } else {
            "已恢复："
        },
        changed
            .iter()
            .map(ClassToken::tag)
            .collect::<Vec<_>>()
            .join("、")
    );
    Ok(())
}

fn limit(
    args: &mut Args,
    store: &ClassTokenStore,
    zone: i32,
    err: &mut dyn Write,
    clock: Clock,
) -> Result<(), CliError> {
    let all = args.flag("--all");
    let days = args.option("--days")?;
    let hours = args.option("--hours")?;
    let at = args.option("--expires")?;
    let expires = expiry(
        days.as_deref(),
        hours.as_deref(),
        at.as_deref(),
        zone,
        clock(),
    )?;
    let daily = match args.option("--daily")? {
        Some(value) => Some(limit_value("--daily", &value)?),
        None => None,
    };
    let total = match args.option("--total")? {
        Some(value) => Some(limit_value("--total", &value)?),
        None => None,
    };
    if expires.is_none() && daily.is_none() && total.is_none() {
        return Err(CliError::Usage(
            "limit 需要 --daily、--total 或过期时间".to_string(),
        ));
    }
    if !all && args.rest.is_empty() {
        return Err(CliError::Usage(
            "limit 需要标签、令牌、前缀或 --all".to_string(),
        ));
    }
    let selectors = args.rest.clone();
    let mut changed: Vec<ClassToken> = Vec::new();
    store.update(|tokens| {
        let targets: Vec<String> = if all {
            tokens.iter().map(|token| token.hash.clone()).collect()
        } else {
            let mut hashes = Vec::new();
            for selector in &selectors {
                hashes.extend(
                    select(&tokens, selector)?
                        .iter()
                        .map(|token| token.hash.clone()),
                );
            }
            hashes
        };
        let mut next = Vec::with_capacity(tokens.len());
        for token in tokens {
            if !targets.contains(&token.hash) {
                next.push(token);
                continue;
            }
            let updated = ClassToken {
                daily_limit: daily.unwrap_or(token.daily_limit),
                total_limit: total.unwrap_or(token.total_limit),
                expires_at: expires.or(token.expires_at),
                ..token
            };
            changed.push(updated.clone());
            next.push(updated);
        }
        Ok(next)
    })?;
    for token in changed {
        let _ = writeln!(
            err,
            "{}：每日上限 {}，总上限 {}，过期 {}",
            token.tag(),
            amount(token.daily_limit),
            amount(token.total_limit),
            time_text(token.expires_at, zone)
        );
    }
    Ok(())
}

fn claims(
    args: &mut Args,
    store: &ClassTokenStore,
    zone: i32,
    out: &mut dyn Write,
) -> Result<(), CliError> {
    if !args.rest.is_empty() {
        return Err(CliError::Usage("claims 不需要参数".to_string()));
    }
    let mut claimed: Vec<ClassToken> = store
        .all()
        .into_iter()
        .filter(|token| token.claim.is_some())
        .collect();
    claimed.sort_by_key(|token| {
        token
            .claim
            .as_ref()
            .map(|claim| claim.claimed_at)
            .unwrap_or(0)
    });
    if claimed.is_empty() {
        let _ = writeln!(out, "（还没有人自助领取：{}）", store.path().display());
        return Ok(());
    }
    let _ = writeln!(
        out,
        "{}",
        [
            "论坛用户名",
            "论坛ID",
            "前缀",
            "状态",
            "今日/每日上限",
            "累计/总上限",
            "请求数",
            "领取时间",
            "过期时间"
        ]
        .join("\t")
    );
    let today = store.day();
    for token in &claimed {
        let claim = token.claim.clone().expect("filtered to claims");
        let _ = writeln!(
            out,
            "{}",
            [
                claim.username.clone(),
                claim.user_id.to_string(),
                token.prefix.clone(),
                if claim.reset {
                    "已重置".to_string()
                } else {
                    state_text(store, token)
                },
                format!("{}/{}", token.used_on(&today), amount(token.daily_limit)),
                format!("{}/{}", token.used_total, amount(token.total_limit)),
                token.requests.to_string(),
                time_text(Some(claim.claimed_at), zone),
                time_text(token.expires_at, zone),
            ]
            .join("\t")
        );
    }
    let active = active_claims(&claimed);
    let _ = writeln!(
        out,
        "已领取 {} 份（另有 {} 条已重置），其中可用 {} 份，累计用量 {} token",
        active.len(),
        claimed.len() - active.len(),
        active
            .iter()
            .filter(|token| store.check(token).is_none())
            .count(),
        claimed.iter().map(|token| token.used_total).sum::<i64>()
    );
    Ok(())
}

fn reset_claim(
    args: &mut Args,
    store: &ClassTokenStore,
    err: &mut dyn Write,
) -> Result<(), CliError> {
    if args.rest.is_empty() {
        return Err(CliError::Usage(
            "reset-claim 需要论坛用户名或论坛用户 ID".to_string(),
        ));
    }
    let selectors = args.rest.clone();
    let mut changed: Vec<ClassToken> = Vec::new();
    store.update(|tokens| {
        let active = active_claims(&tokens);
        let mut targets: Vec<String> = Vec::new();
        for selector in &selectors {
            let key = selector.trim();
            let key = key.strip_prefix(LABEL).unwrap_or(key);
            let key = key.strip_prefix('@').unwrap_or(key);
            let matched: Vec<&ClassToken> = active
                .iter()
                .filter(|token| {
                    token.claim.as_ref().is_some_and(|claim| {
                        claim.username.eq_ignore_ascii_case(key) || claim.user_id.to_string() == key
                    })
                })
                .collect();
            if matched.is_empty() {
                return Err(StoreError::Rejected(format!(
                    "找不到「{key}」的有效领取记录（claims 查看）"
                )));
            }
            targets.extend(matched.iter().map(|token| token.hash.clone()));
        }
        let mut next = Vec::with_capacity(tokens.len());
        for token in tokens {
            if !targets.contains(&token.hash) {
                next.push(token);
                continue;
            }
            let updated = ClassToken {
                revoked: true,
                claim: token.claim.map(|claim| ForumClaim {
                    reset: true,
                    ..claim
                }),
                ..token
            };
            changed.push(updated.clone());
            next.push(updated);
        }
        Ok(next)
    })?;
    for token in changed {
        let _ = writeln!(
            err,
            "已重置 {}：原令牌已作废，该论坛账号可以重新领取",
            token.tag()
        );
    }
    Ok(())
}

/// `学员 07` when the label starts with `学员 ` and the rest is a number.
fn labelled_number(label: &str, prefix: &str) -> Option<i64> {
    let rest = label.strip_prefix(prefix)?;
    if rest.is_empty() || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn select<'a>(tokens: &'a [ClassToken], selector: &str) -> Result<Vec<&'a ClassToken>, StoreError> {
    let text = selector.trim();
    let compact: String = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let mut found: Vec<&ClassToken> = Vec::new();
    if ClassTokenCodec::looks_like_token(text) {
        let hash = ClassTokenCodec::hash(text);
        found.extend(tokens.iter().filter(|token| token.hash == hash));
    } else {
        found.extend(tokens.iter().filter(|token| token.label == text));
        if found.is_empty() {
            found.extend(tokens.iter().filter(|token| {
                token
                    .label
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .collect::<String>()
                    == compact
            }));
        }
        if found.is_empty() && text.starts_with(PREFIX) && text.chars().count() >= 4 {
            let head: String = text.chars().take(6).collect();
            found.extend(tokens.iter().filter(|token| {
                token.prefix.starts_with(&head)
                    && head.chars().count() <= token.prefix.chars().count()
            }));
        }
    }
    if found.is_empty() {
        return Err(StoreError::Rejected(format!("找不到令牌：{text}")));
    }
    if found.len() > 1 && !found.iter().all(|token| token.label == found[0].label) {
        let tags = found
            .iter()
            .map(|token| token.tag())
            .collect::<Vec<_>>()
            .join("、");
        return Err(StoreError::Rejected(format!(
            "「{text}」对应多个令牌（{tags}），请用标签或完整令牌"
        )));
    }
    Ok(found)
}

fn state_text(store: &ClassTokenStore, token: &ClassToken) -> String {
    match store.check(token) {
        None => "可用".to_string(),
        Some(denied) => match denied.code.as_str() {
            "token_revoked" => "已作废".to_string(),
            "token_expired" => "已过期".to_string(),
            "total_quota_exceeded" => "总额度用完".to_string(),
            _ => "今日额度用完".to_string(),
        },
    }
}

/// `--daily`/`--total`: `None` when absent, `Some(None)` for `none` (unlimited).
fn limit_value(name: &str, value: &str) -> Result<Option<i64>, CliError> {
    let lower = value.to_lowercase();
    if matches!(lower.as_str(), "none" | "off" | "无" | "不限") {
        return Ok(None);
    }
    let digits: String = value
        .chars()
        .filter(|character| *character != '_' && *character != ',')
        .collect();
    digits
        .parse::<i64>()
        .ok()
        .filter(|number| *number > 0)
        .map(Some)
        .ok_or_else(|| CliError::Usage(format!("{name} 需要正整数或 none")))
}

/// `--days`, `--hours` or `--expires`; at most one may be given.
fn expiry(
    days: Option<&str>,
    hours: Option<&str>,
    at: Option<&str>,
    zone: i32,
    now: i64,
) -> Result<Option<i64>, CliError> {
    let given = [days, hours, at]
        .iter()
        .filter(|value| value.is_some())
        .count();
    if given > 1 {
        return Err(CliError::Usage(
            "--days、--hours、--expires 只能选一个".to_string(),
        ));
    }
    if let Some(days) = days {
        let days = days
            .parse::<f64>()
            .ok()
            .filter(|days| *days > 0.0)
            .ok_or_else(|| CliError::Usage("--days 需要正数".to_string()))?;
        return Ok(Some(after_days(now, days)));
    }
    if let Some(hours) = hours {
        let hours = hours
            .parse::<f64>()
            .ok()
            .filter(|hours| *hours > 0.0)
            .ok_or_else(|| CliError::Usage("--hours 需要正数".to_string()))?;
        return Ok(Some(after_hours(now, hours)));
    }
    if let Some(at) = at {
        let text = at.trim();
        let local = text.replace(' ', "T");
        let local = if local.len() == 16 {
            format!("{local}:00")
        } else {
            local
        };
        let parsed = chrono::DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|parsed| parsed.timestamp_millis())
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(&local, "%Y-%m-%dT%H:%M:%S")
                    .ok()
                    .and_then(|naive| naive.and_local_timezone(offset(zone)).single())
                    .map(|at| at.timestamp_millis())
            });
        return parsed
            .map(Some)
            .ok_or_else(|| CliError::Usage("--expires 格式：2026-11-08 22:00".to_string()));
    }
    Ok(None)
}

fn zone_offset(text: &str) -> Result<i32, CliError> {
    crate::class_tokens::zone_offset(text).ok_or_else(|| {
        CliError::Usage(format!(
            "不支持的时区 {text}（可用 +08:00 这样的偏移或 Asia/Shanghai）"
        ))
    })
}

fn amount(value: Option<i64>) -> String {
    value.map_or_else(|| "不限".to_string(), |value| value.to_string())
}

fn time_text(value: Option<i64>, zone: i32) -> String {
    value
        .filter(|millis| *millis > 0)
        .map_or_else(|| "-".to_string(), |millis| display(millis, zone))
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// The binary's entry point: the process arguments, the environment and the system clock.
pub fn run_from_env() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let clock: Clock = std::sync::Arc::new(xiaoman_api::now_millis);
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    run(
        &args,
        &mut out,
        &mut err,
        &|name| std::env::var(name).ok(),
        clock,
    )
}
