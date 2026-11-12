//! Command line, environment and `$XIAOMAN_HOME/config.properties`, merged in that order.
//!
//! API keys are read from the environment only: `XIAOMAN_CHAT_API_KEY` / `XIAOMAN_OPERATOR_API_KEY`,
//! falling back to `ARK_API_KEY`. A key found in `config.properties` is ignored with a warning.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use clap::{Arg, ArgAction, ArgMatches, Command as ClapCommand};
use xiaoman_api::{ModelConfig, ModelRole};

pub const DEFAULT_WDA_URL: &str = "http://127.0.0.1:8100";
pub const DEFAULT_BASE_URL: &str = "https://ark.cn-beijing.volces.com/api/v3";
pub const DEFAULT_MAX_STEPS: i32 = 30;
/// `config.properties` names containing one of these words hold secrets and are ignored.
const SECRET_WORDS: [&str; 4] = ["key", "secret", "token", "password"];

/// The `ios-runner` help text, printed for `--help` and after a usage error.
pub const USAGE: &str = "ios-runner：在 Mac 上通过 WebDriverAgent 操作 iPhone 的小满。

用法：
  ios-runner check [--wda URL]                     检查 WDA 连接、截图和界面节点
  ios-runner dump --out DIR [--wda URL]            保存截图、原始 source.xml 和节点列表
  ios-runner task \"目标\" [--skill 名称] [--max-steps N]   执行一个手机任务
  ios-runner chat                                  交互聊天，输入 /exit 退出
  ios-runner ask \"问题\"                             问一句就结束（会用到 MCP 工具）
  ios-runner tools [--mcp FILE]                    列出 MCP 服务器和工具
  ios-runner call 工具名 '{\"参数\":\"值\"}'            直接调用一个 MCP 工具（不经过模型，用于排查）

通用参数：
  --wda URL           WDA 地址，默认 XIAOMAN_WDA_URL 或 http://127.0.0.1:8100
                      不插线时用手机的局域网地址：scripts/ios/wda.sh url
  --home DIR          数据目录（memory/ skills/ traces/ config.properties），默认 XIAOMAN_HOME 或 ~/.xiaoman-ios
  --skills DIR        追加技能目录，可重复，例如仓库里的 skills/ios
  --non-interactive   不读终端输入：需要确认的操作一律拒绝
  --max-steps N       单个任务的最大步数，默认 30
  --mcp FILE          MCP 服务器配置，默认 XIAOMAN_HOME/mcp.json（格式见 mcp/servers.example.json）

任务进行中按 Ctrl-C 会在下一步前停止任务，再按一次退出程序。
模型：config.properties 写 chat.baseUrl、chat.model、operator.baseUrl、operator.model；
环境变量 XIAOMAN_CHAT_MODEL / XIAOMAN_OPERATOR_MODEL / XIAOMAN_CHAT_BASE_URL / XIAOMAN_OPERATOR_BASE_URL 可覆盖。
API Key 只从环境变量读取：XIAOMAN_CHAT_API_KEY、XIAOMAN_OPERATOR_API_KEY，缺省用 ARK_API_KEY。";

/// The command line could not be used: `Help` prints `USAGE` and exits 0, `Usage` exits 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    Help(String),
    Usage(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Help(text) | Self::Usage(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for CliError {}

/// What the user asked ios-runner to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Check,
    Dump,
    Task,
    Chat,
    Ask,
    Tools,
    Call,
}

impl Command {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Dump => "dump",
            Self::Task => "task",
            Self::Chat => "chat",
            Self::Ask => "ask",
            Self::Tools => "tools",
            Self::Call => "call",
        }
    }
}

/// One model endpoint. The key comes from the environment only and never appears in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct ModelSettings {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

impl ModelSettings {
    pub fn configured(&self) -> bool {
        !self.base_url.trim().is_empty()
            && !self.model.trim().is_empty()
            && !self.api_key.trim().is_empty()
    }

    pub fn to_model_config(&self, role: ModelRole) -> ModelConfig {
        ModelConfig::new(
            role,
            self.base_url.clone(),
            self.model.clone(),
            self.api_key.clone(),
        )
    }
}

impl fmt::Debug for ModelSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = if self.api_key.is_empty() {
            "<unset>"
        } else {
            "***"
        };
        f.debug_struct("ModelSettings")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &key)
            .finish()
    }
}

/// Command line, environment and `config.properties` merged into one run.
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub command: Command,
    /// Goal of `task`, question of `ask`, tool name of `call`.
    pub goal: Option<String>,
    /// JSON arguments of `call`.
    pub tool_arguments: String,
    pub skill: Option<String>,
    pub out_dir: Option<PathBuf>,
    /// MCP servers file; `$XIAOMAN_HOME/mcp.json` unless `--mcp` is given.
    pub mcp_file: Option<PathBuf>,
    pub wda_url: String,
    pub home: PathBuf,
    pub extra_skill_dirs: Vec<PathBuf>,
    pub non_interactive: bool,
    pub max_steps: i32,
    /// `WDA_BUNDLE_ID` shared with `scripts/ios/wda.sh`; `None` means the script default.
    pub wda_bundle_id: Option<String>,
    pub chat: ModelSettings,
    pub operator: ModelSettings,
    pub warnings: Vec<String>,
}

impl RunnerConfig {
    pub fn memory_dir(&self) -> PathBuf {
        self.home.join("memory")
    }

    pub fn user_skills_dir(&self) -> PathBuf {
        self.home.join("skills")
    }

    pub fn traces_dir(&self) -> PathBuf {
        self.home.join("traces")
    }

    pub fn mcp_config_file(&self) -> PathBuf {
        self.mcp_file
            .clone()
            .unwrap_or_else(|| self.home.join("mcp.json"))
    }

    /// Extra skill folders first, the user's own skills last so they override.
    pub fn skill_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = self.extra_skill_dirs.clone();
        dirs.push(self.user_skills_dir());
        dirs
    }

    /// Configured API keys, used to keep them out of traces.
    pub fn secrets(&self) -> Vec<String> {
        let mut secrets: Vec<String> = Vec::new();
        for key in [&self.chat.api_key, &self.operator.api_key] {
            if !key.trim().is_empty() && !secrets.contains(key) {
                secrets.push(key.clone());
            }
        }
        secrets
    }

    /// Chinese explanation of what is missing for task/chat, or `None` when both models are usable.
    pub fn missing_models(&self) -> Option<String> {
        let mut missing: Vec<&str> = Vec::new();
        if self.chat.model.trim().is_empty() {
            missing.push("chat.model（或环境变量 XIAOMAN_CHAT_MODEL）");
        }
        if self.operator.model.trim().is_empty() {
            missing.push("operator.model（或环境变量 XIAOMAN_OPERATOR_MODEL）");
        }
        if self.chat.api_key.trim().is_empty() {
            missing.push("环境变量 XIAOMAN_CHAT_API_KEY 或 ARK_API_KEY");
        }
        if self.operator.api_key.trim().is_empty() {
            missing.push("环境变量 XIAOMAN_OPERATOR_API_KEY 或 ARK_API_KEY");
        }
        if self.chat.base_url.trim().is_empty() {
            missing.push("chat.baseUrl");
        }
        if missing.is_empty() {
            return None;
        }
        Some(format!(
            "模型还没配置好，缺少：{}。模型名写在 {}，Key 只用环境变量传入。",
            missing.join("、"),
            self.home.join("config.properties").display()
        ))
    }

    /// Parses the command line, then merges environment variables and `config.properties`.
    pub fn parse(
        args: &[String],
        env: &BTreeMap<String, String>,
        user_home: &Path,
    ) -> Result<Self, CliError> {
        if args.iter().any(|arg| arg == "-h" || arg == "--help") {
            return Err(CliError::Help(USAGE.to_string()));
        }
        if args.is_empty() {
            return Err(CliError::Usage(String::new()));
        }
        let matches = cli()
            .try_get_matches_from(
                std::iter::once("ios-runner".to_string()).chain(args.iter().cloned()),
            )
            .map_err(|error| CliError::Usage(error.render().to_string().trim().to_string()))?;
        let Some((name, sub)) = matches.subcommand() else {
            return Err(CliError::Usage(String::new()));
        };

        let (command, goal, tool_arguments) = match name {
            "check" => (Command::Check, None, "{}".to_string()),
            "dump" => (Command::Dump, None, "{}".to_string()),
            "task" => (Command::Task, positional(sub, "goal"), "{}".to_string()),
            "chat" => (Command::Chat, None, "{}".to_string()),
            "ask" => (Command::Ask, positional(sub, "question"), "{}".to_string()),
            "tools" => (Command::Tools, None, "{}".to_string()),
            "call" => (
                Command::Call,
                positional(sub, "tool"),
                positional(sub, "arguments").unwrap_or_else(|| "{}".to_string()),
            ),
            other => return Err(CliError::Usage(format!("未知命令：{other}"))),
        };

        let out = option(&matches, sub, "out").map(PathBuf::from);
        if command == Command::Dump && out.is_none() {
            return Err(CliError::Usage("dump 需要 --out DIR".to_string()));
        }
        let max_steps = integer(&matches, sub, "max-steps").unwrap_or(DEFAULT_MAX_STEPS);

        let home = option(&matches, sub, "home")
            .map(PathBuf::from)
            .or_else(|| env_string(env, "XIAOMAN_HOME").map(PathBuf::from))
            .unwrap_or_else(|| user_home.join(".xiaoman-ios"));
        let mut warnings: Vec<String> = Vec::new();
        let properties = load_properties(&home.join("config.properties"), &mut warnings);
        let setting = |env_name: &str, property: &str, default: &str| -> String {
            env_string(env, env_name)
                .or_else(|| properties.get(property).cloned())
                .unwrap_or_else(|| default.to_string())
        };

        let chat_base = setting("XIAOMAN_CHAT_BASE_URL", "chat.baseUrl", DEFAULT_BASE_URL);
        let shared_key = env_string(env, "ARK_API_KEY").unwrap_or_default();
        let chat_key =
            env_string(env, "XIAOMAN_CHAT_API_KEY").unwrap_or_else(|| shared_key.clone());
        let operator_key =
            env_string(env, "XIAOMAN_OPERATOR_API_KEY").unwrap_or_else(|| shared_key.clone());

        Ok(Self {
            command,
            goal,
            tool_arguments,
            skill: option(&matches, sub, "skill"),
            out_dir: out,
            mcp_file: option(&matches, sub, "mcp").map(PathBuf::from),
            wda_url: option(&matches, sub, "wda")
                .or_else(|| env_string(env, "XIAOMAN_WDA_URL"))
                .unwrap_or_else(|| DEFAULT_WDA_URL.to_string()),
            home,
            extra_skill_dirs: values(&matches, sub, "skills")
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            non_interactive: flag(&matches, sub, "non-interactive"),
            max_steps,
            wda_bundle_id: env_string(env, "WDA_BUNDLE_ID"),
            chat: ModelSettings {
                base_url: chat_base.clone(),
                model: setting("XIAOMAN_CHAT_MODEL", "chat.model", ""),
                api_key: chat_key,
            },
            operator: ModelSettings {
                base_url: setting("XIAOMAN_OPERATOR_BASE_URL", "operator.baseUrl", &chat_base),
                model: setting("XIAOMAN_OPERATOR_MODEL", "operator.model", ""),
                api_key: operator_key,
            },
            warnings,
        })
    }
}

/// The command line, before values are merged with the environment.
fn cli() -> ClapCommand {
    let option = |name: &'static str, value_name: &'static str| {
        Arg::new(name)
            .long(name)
            .value_name(value_name)
            .num_args(1)
            .global(true)
    };
    ClapCommand::new("ios-runner")
        .about("在 Mac 上通过 WebDriverAgent 操作 iPhone 的小满")
        .disable_help_flag(true)
        .disable_version_flag(true)
        .subcommand_required(true)
        .arg(option("wda", "URL"))
        .arg(option("home", "DIR"))
        .arg(option("out", "DIR"))
        .arg(option("mcp", "FILE"))
        .arg(option("skill", "名称"))
        .arg(
            Arg::new("skills")
                .long("skills")
                .value_name("DIR")
                .num_args(1)
                .action(ArgAction::Append)
                .global(true),
        )
        .arg(
            Arg::new("max-steps")
                .long("max-steps")
                .value_name("N")
                .num_args(1)
                .value_parser(clap::value_parser!(i32).range(1..=200))
                .global(true),
        )
        .arg(
            Arg::new("non-interactive")
                .long("non-interactive")
                .num_args(0)
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .subcommand(ClapCommand::new("check").about("检查 WDA 连接、截图和界面节点"))
        .subcommand(ClapCommand::new("dump").about("保存截图、原始 source.xml 和节点列表"))
        .subcommand(
            ClapCommand::new("task").about("执行一个手机任务").arg(
                Arg::new("goal")
                    .value_name("目标")
                    .required(true)
                    .num_args(1),
            ),
        )
        .subcommand(ClapCommand::new("chat").about("交互聊天，输入 /exit 退出"))
        .subcommand(
            ClapCommand::new("ask").about("问一句就结束").arg(
                Arg::new("question")
                    .value_name("问题")
                    .required(true)
                    .num_args(1),
            ),
        )
        .subcommand(ClapCommand::new("tools").about("列出 MCP 服务器和工具"))
        .subcommand(
            ClapCommand::new("call")
                .about("直接调用一个 MCP 工具")
                .arg(
                    Arg::new("tool")
                        .value_name("工具名")
                        .required(true)
                        .num_args(1),
                )
                .arg(Arg::new("arguments").value_name("JSON参数").num_args(1)),
        )
}

fn option(matches: &ArgMatches, sub: &ArgMatches, name: &str) -> Option<String> {
    sub.try_get_one::<String>(name)
        .ok()
        .flatten()
        .or_else(|| matches.try_get_one::<String>(name).ok().flatten())
        .cloned()
}

fn integer(matches: &ArgMatches, sub: &ArgMatches, name: &str) -> Option<i32> {
    sub.try_get_one::<i32>(name)
        .ok()
        .flatten()
        .or_else(|| matches.try_get_one::<i32>(name).ok().flatten())
        .copied()
}

fn values(matches: &ArgMatches, sub: &ArgMatches, name: &str) -> Vec<String> {
    let from_sub: Vec<String> = sub
        .try_get_many::<String>(name)
        .ok()
        .flatten()
        .map(|values| values.cloned().collect())
        .unwrap_or_default();
    if !from_sub.is_empty() {
        return from_sub;
    }
    matches
        .try_get_many::<String>(name)
        .ok()
        .flatten()
        .map(|values| values.cloned().collect())
        .unwrap_or_default()
}

fn flag(matches: &ArgMatches, sub: &ArgMatches, name: &str) -> bool {
    sub.try_get_one::<bool>(name)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
        || matches
            .try_get_one::<bool>(name)
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false)
}

fn positional(sub: &ArgMatches, name: &str) -> Option<String> {
    sub.try_get_one::<String>(name).ok().flatten().cloned()
}

fn env_string(env: &BTreeMap<String, String>, name: &str) -> Option<String> {
    env.get(name)
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

/// Minimal `java.util.Properties` reader: `key=value` / `key: value`, `#` and `!` comments.
fn load_properties(path: &Path, warnings: &mut Vec<String>) -> HashMap<String, String> {
    let mut properties = HashMap::new();
    if !path.is_file() {
        return properties;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            warnings.push(format!("无法读取 {}：{error}", path.display()));
            return properties;
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        let Some(split) = line.find(['=', ':']) else {
            continue;
        };
        let name = line[..split].trim();
        let value = line[split + 1..].trim();
        if name.is_empty() {
            continue;
        }
        if SECRET_WORDS
            .iter()
            .any(|word| name.to_lowercase().contains(word))
        {
            warnings.push(format!(
                "{} 里的 {name} 已忽略：Key 只从环境变量读取，请删掉文件里的这一行。",
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
            ));
            continue;
        }
        if value.is_empty() {
            continue;
        }
        properties.insert(name.to_string(), value.to_string());
    }
    properties
}
