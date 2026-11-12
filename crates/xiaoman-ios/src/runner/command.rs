//! The `ios-runner` command: `check`, `dump`, `task`, `chat`, `ask`, `tools` and `call`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use xiaoman_agent::XiaomanAgent;
use xiaoman_agent::screen_text;
use xiaoman_api::{Agent, AgentEvent, AgentListener, Device, ScreenState, StopSignal};
use xiaoman_mcp::{McpClient, McpConfig, transport};

use super::assembly::{LlmFactory, RunnerSession, assemble, default_llm_factory, write_line};
use super::channels::{ConsoleChannels, Input, Output};
use super::config::{CliError, Command, RunnerConfig, USAGE};
use crate::wda_client::{WdaClient, json_content};
use crate::wda_device::{WdaDevice, WdaDeviceOptions};

/// First Ctrl-C stops the running task before its next step; a second one exits.
#[derive(Default)]
pub struct CtrlCStop {
    flag: AtomicBool,
}

impl CtrlCStop {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&self) {
        self.flag.store(false, Ordering::SeqCst);
    }

    /// Returns true when this press only stopped the task.
    pub fn press(&self) -> bool {
        self.flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

impl StopSignal for CtrlCStop {
    fn stopped(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// Hooks Ctrl-C: the first press stops the task, the second exits the process.
pub fn install_ctrl_c(stop: Arc<CtrlCStop>) {
    tokio::spawn(async move {
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
            if !stop.press() {
                std::process::exit(130);
            }
            eprintln!("\n正在停止当前任务……再按一次 Ctrl-C 退出。");
        }
    });
}

/// The `ios-runner` command line; everything outside the process (env, streams, models)
/// is injectable for tests.
pub struct Runner {
    env: BTreeMap<String, String>,
    input: Input,
    out: Output,
    err: Output,
    has_console: bool,
    llm: LlmFactory,
    user_home: PathBuf,
    stop: Arc<CtrlCStop>,
}

impl Default for Runner {
    fn default() -> Self {
        Self::new()
    }
}

impl Runner {
    pub fn new() -> Self {
        Self {
            env: std::env::vars().collect(),
            input: Arc::new(Mutex::new(Box::new(BufReader::new(std::io::stdin())))),
            out: Arc::new(Mutex::new(Box::new(std::io::stdout()))),
            err: Arc::new(Mutex::new(Box::new(std::io::stderr()))),
            has_console: std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
            llm: default_llm_factory(),
            user_home: user_home(),
            stop: Arc::new(CtrlCStop::new()),
        }
    }

    pub fn with_env(mut self, env: BTreeMap<String, String>) -> Self {
        self.env = env;
        self
    }

    pub fn with_input(mut self, input: Box<dyn BufRead + Send>) -> Self {
        self.input = Arc::new(Mutex::new(input));
        self
    }

    pub fn with_out(mut self, out: Box<dyn Write + Send>) -> Self {
        self.out = Arc::new(Mutex::new(out));
        self
    }

    pub fn with_err(mut self, err: Box<dyn Write + Send>) -> Self {
        self.err = Arc::new(Mutex::new(err));
        self
    }

    pub fn with_console(mut self, has_console: bool) -> Self {
        self.has_console = has_console;
        self
    }

    pub fn with_llm(mut self, llm: LlmFactory) -> Self {
        self.llm = llm;
        self
    }

    pub fn with_user_home(mut self, user_home: impl Into<PathBuf>) -> Self {
        self.user_home = user_home.into();
        self
    }

    pub fn with_stop(mut self, stop: Arc<CtrlCStop>) -> Self {
        self.stop = stop;
        self
    }

    pub async fn run(&self, args: Vec<String>) -> i32 {
        if args.is_empty() {
            self.err_line(USAGE);
            return 2;
        }
        let config = match RunnerConfig::parse(&args, &self.env, &self.user_home) {
            Ok(config) => config,
            Err(CliError::Help(text)) => {
                self.out_line(&text);
                return 0;
            }
            Err(CliError::Usage(message)) => {
                if !message.is_empty() {
                    self.err_line(&format!("参数错误：{message}"));
                }
                self.err_line(USAGE);
                return 2;
            }
        };
        for warning in &config.warnings {
            self.err_line(&format!("注意：{warning}"));
        }
        match config.command {
            Command::Check => self.check(&config).await,
            Command::Dump => self.dump(&config).await,
            Command::Tools => self.tools(&config).await,
            Command::Call => self.call(&config).await,
            Command::Task | Command::Chat | Command::Ask => self.agent_command(&config).await,
        }
    }

    async fn check(&self, config: &RunnerConfig) -> i32 {
        match self.check_result(config).await {
            Ok(code) => code,
            Err(error) => self.wda_failure(config, &error),
        }
    }

    async fn check_result(&self, config: &RunnerConfig) -> Result<i32, String> {
        let (client, device) = self.open_device(config).await?;
        let status = client.status().await.map_err(|error| error.to_string())?;
        let ready = status
            .get("ready")
            .and_then(json_content)
            .unwrap_or_else(|| "未知".to_string());
        let os_version = status
            .get("os")
            .and_then(|os| os.get("version"))
            .and_then(json_content);
        let os = os_version
            .map(|version| format!("，iOS {version}"))
            .unwrap_or_default();
        self.out_line(&format!("WDA：{}（ready={ready}{os}）", config.wda_url));
        let screen = device.observe().await.map_err(|error| error.to_string())?;
        self.print_screen(&screen);
        self.out_line("连接正常。");
        Ok(0)
    }

    async fn dump(&self, config: &RunnerConfig) -> i32 {
        match self.dump_result(config).await {
            Ok(code) => code,
            Err(error) => self.wda_failure(config, &error),
        }
    }

    async fn dump_result(&self, config: &RunnerConfig) -> Result<i32, String> {
        let (client, device) = self.open_device(config).await?;
        let dir = config
            .out_dir
            .clone()
            .ok_or_else(|| "dump 需要 --out DIR".to_string())?;
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("无法创建 {}：{error}", dir.display()))?;
        let screen = device.observe().await.map_err(|error| error.to_string())?;
        if let Some(png) = &screen.screenshot_png {
            std::fs::write(dir.join("screenshot.png"), png)
                .map_err(|error| format!("无法写入 screenshot.png：{error}"))?;
        }
        let source = client.source().await.map_err(|error| error.to_string())?;
        std::fs::write(dir.join("source.xml"), source)
            .map_err(|error| format!("无法写入 source.xml：{error}"))?;
        let nodes = format!(
            "{}\n",
            screen
                .nodes
                .iter()
                .map(screen_text::node_line)
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::fs::write(dir.join("nodes.txt"), nodes)
            .map_err(|error| format!("无法写入 nodes.txt：{error}"))?;
        self.print_screen(&screen);
        let absolute = std::path::absolute(&dir).unwrap_or(dir);
        self.out_line(&format!(
            "已保存到 {}：screenshot.png、source.xml、nodes.txt",
            absolute.display()
        ));
        Ok(0)
    }

    async fn tools(&self, config: &RunnerConfig) -> i32 {
        let mcp = McpConfig::load(&config.mcp_config_file(), &self.env);
        for warning in &mcp.warnings {
            self.out_line(&format!("MCP：{warning}"));
        }
        let servers: Vec<_> = mcp.servers.iter().filter(|server| server.enabled).collect();
        if servers.is_empty() {
            self.out_line(&format!(
                "没有可用的 MCP 服务器。把 mcp/servers.example.json 复制为 {}，按需填好 Key 对应的环境变量。",
                config.mcp_config_file().display()
            ));
            return 1;
        }
        let mut tools = 0usize;
        for server in servers {
            match transport::create(server) {
                Ok(transport) => {
                    let client = McpClient::new(server.clone(), transport);
                    match client.list_tools().await {
                        Ok(list) => {
                            tools += list.len();
                            self.out_line(&format!(
                                "{}（{}）：已连接，{} 个工具",
                                server.title,
                                server.name,
                                list.len()
                            ));
                            for tool in list {
                                self.out_line(&format!(
                                    "  {}__{}  {}",
                                    server.name,
                                    tool.name,
                                    truncate_chars(
                                        tool.description.as_deref().unwrap_or_default(),
                                        60
                                    )
                                ));
                            }
                        }
                        Err(error) => {
                            self.out_line(&format!(
                                "{}（{}）：连接失败：{error}",
                                server.title, server.name
                            ));
                        }
                    }
                    client.close();
                }
                Err(error) => {
                    self.out_line(&format!(
                        "{}（{}）：连接失败：{error}",
                        server.title, server.name
                    ));
                }
            }
        }
        if tools == 0 { 1 } else { 0 }
    }

    async fn call(&self, config: &RunnerConfig) -> i32 {
        let tool = config.goal.clone().unwrap_or_default();
        let Some((server_name, tool_name)) = tool.split_once("__") else {
            self.err_line(&format!(
                "没有这个工具：{tool}。用 ios-runner tools 查看工具名。"
            ));
            return 1;
        };
        let mcp = McpConfig::load(&config.mcp_config_file(), &self.env);
        let Some(server) = mcp
            .servers
            .iter()
            .find(|server| server.enabled && server.name == server_name)
        else {
            self.err_line(&format!(
                "没有这个工具：{tool}。用 ios-runner tools 查看工具名。"
            ));
            return 1;
        };
        let arguments: Value = match serde_json::from_str(&config.tool_arguments) {
            Ok(arguments) => arguments,
            Err(error) => {
                self.err_line(&format!("参数错误：{error}"));
                return 1;
            }
        };
        let transport = match transport::create(server) {
            Ok(transport) => transport,
            Err(error) => {
                self.err_line(&format!("连接失败：{error}"));
                return 1;
            }
        };
        let client = McpClient::new(server.clone(), transport);
        let result = client.call_tool(tool_name, arguments).await;
        client.close();
        match result {
            Ok(result) => {
                self.out_line(&result.text);
                if result.ok { 0 } else { 1 }
            }
            Err(error) => {
                self.err_line(&format!("调用失败：{error}"));
                1
            }
        }
    }

    async fn agent_command(&self, config: &RunnerConfig) -> i32 {
        if let Some(message) = config.missing_models() {
            self.err_line(&message);
            return 2;
        }
        let channels = Arc::new(ConsoleChannels::from_shared(
            self.input.clone(),
            self.out.clone(),
            self.has_console && !config.non_interactive,
        ));
        let session = match assemble(
            config,
            channels.clone(),
            &self.llm,
            &self.env,
            self.out.clone(),
        )
        .await
        {
            Ok(session) => session,
            Err(error) => {
                self.err_line(&format!("初始化失败：{error}"));
                return 1;
            }
        };
        let trace_dir = std::path::absolute(session.trace.dir())
            .unwrap_or_else(|_| session.trace.dir().to_path_buf());
        self.out_line(&format!(
            "技能：{} 个；轨迹目录：{}",
            session.skill_count,
            trace_dir.display()
        ));
        if !channels.interactive() {
            self.out_line("非交互模式：需要确认的操作会被自动拒绝。");
        }
        let listener = self.listener(&session);
        self.converse(config, &session.agent, listener).await
    }

    fn listener(&self, session: &RunnerSession) -> Arc<dyn AgentListener> {
        let trace = session.trace.clone();
        let sentinel = session.sentinel.clone();
        let out = self.out.clone();
        Arc::new(move |event: &AgentEvent| {
            trace.event(event);
            print_event(&out, event);
            if let AgentEvent::PhoneTaskFinished { task_id, .. } = event {
                sentinel.end_task(task_id);
            }
        })
    }

    async fn converse(
        &self,
        config: &RunnerConfig,
        agent: &XiaomanAgent,
        listener: Arc<dyn AgentListener>,
    ) -> i32 {
        let goal = config.goal.clone().unwrap_or_default();
        match config.command {
            Command::Ask => {
                self.stop.reset();
                agent.handle(&goal, listener, self.stop.clone()).await;
                0
            }
            Command::Task => {
                self.stop.reset();
                let event = agent
                    .run_phone_task(&goal, config.skill.as_deref(), listener, self.stop.clone())
                    .await;
                match event {
                    AgentEvent::PhoneTaskFinished { success: true, .. } => 0,
                    _ => 1,
                }
            }
            _ => {
                self.out_line("开始聊天，输入 /exit 退出；任务进行中按 Ctrl-C 停止当前任务。");
                loop {
                    self.out("你 > ");
                    let Some(line) = self.read_line().await else {
                        break;
                    };
                    let line = line.trim().to_string();
                    if line == "/exit" {
                        break;
                    }
                    if line.is_empty() {
                        continue;
                    }
                    self.stop.reset();
                    agent
                        .handle(&line, listener.clone(), self.stop.clone())
                        .await;
                }
                0
            }
        }
    }

    async fn open_device(
        &self,
        config: &RunnerConfig,
    ) -> Result<(Arc<WdaClient>, WdaDevice), String> {
        let client =
            Arc::new(WdaClient::new(config.wda_url.clone()).map_err(|error| error.to_string())?);
        let device = WdaDevice::with_options(client.clone(), WdaDeviceOptions::default())
            .map_err(|error| error.to_string())?;
        Ok((client, device))
    }

    fn wda_failure(&self, config: &RunnerConfig, error: &str) -> i32 {
        self.err_line(&format!("连不上 WDA（{}）：{error}", config.wda_url));
        self.err_line(
            "请先在 Mac 上运行 scripts/ios/wda.sh start（插线时再运行 forward），\
             再用 scripts/ios/wda.sh status 确认；手机需保持解锁。",
        );
        1
    }

    fn print_screen(&self, screen: &ScreenState) {
        self.out_line(&format!(
            "屏幕：{}x{} 点，前台 App：{}",
            screen.width_px,
            screen.height_px,
            screen.package_name.as_deref().unwrap_or("未知")
        ));
        let screenshot = match &screen.screenshot_png {
            None => "无".to_string(),
            Some(png) => match image_dimensions(png) {
                Some((width, height)) => format!("{width}x{height}，{} 字节", png.len()),
                None => "无法解码".to_string(),
            },
        };
        self.out_line(&format!("截图：{screenshot}"));
        self.out_line(&format!("节点：{} 个", screen.nodes.len()));
        for node in screen.nodes.iter().take(15) {
            self.out_line(&format!("  {}", screen_text::node_line(node)));
        }
    }

    fn out(&self, text: &str) {
        if let Ok(mut out) = self.out.lock() {
            let _ = out.write_all(text.as_bytes());
            let _ = out.flush();
        }
    }

    fn out_line(&self, text: &str) {
        write_line(&self.out, text);
    }

    fn err_line(&self, text: &str) {
        write_line(&self.err, text);
    }

    async fn read_line(&self) -> Option<String> {
        let input = self.input.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = input.lock().ok()?;
            let mut line = String::new();
            match guard.read_line(&mut line) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
            }
        })
        .await
        .ok()
        .flatten()
    }
}

fn print_event(out: &Output, event: &AgentEvent) {
    let line = match event {
        AgentEvent::Reply { text, .. } => format!("小满：{text}"),
        AgentEvent::SkillChosen { skill_name, .. } => format!("使用技能：{skill_name}"),
        AgentEvent::PhoneTaskStarted {
            goal, skill_name, ..
        } => {
            let skill = skill_name
                .as_ref()
                .map(|skill| format!("（技能 {skill}）"))
                .unwrap_or_default();
            format!("开始手机任务：{goal}{skill}")
        }
        AgentEvent::StepPlanned {
            step_index, action, ..
        } => {
            format!("  [{}] {}", step_index + 1, screen_text::describe(action))
        }
        AgentEvent::StepExecuted { result, .. } => {
            if result.ok {
                return;
            }
            format!(
                "      失败：{}",
                result.message.as_deref().unwrap_or("无说明")
            )
        }
        AgentEvent::StepBlocked { decision, .. } => {
            format!("      被安全模块拦下：{}", decision.reason())
        }
        AgentEvent::NeedsUser { question, .. } => format!("  需要你：{question}"),
        AgentEvent::MemoryUpdated { permalink, .. } => format!("  已更新记忆：{permalink}"),
        AgentEvent::ToolCalled { summary, .. } => format!("  调用工具：{summary}"),
        AgentEvent::PhoneTaskFinished {
            success,
            summary,
            steps,
            ..
        } => {
            let state = if *success {
                "任务完成"
            } else {
                "任务未完成"
            };
            format!("{state}（{steps} 步）：{summary}")
        }
        AgentEvent::Failed { message, .. } => format!("出错：{message}"),
    };
    write_line(out, &line);
}

fn image_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(png))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(".").to_path_buf())
}
