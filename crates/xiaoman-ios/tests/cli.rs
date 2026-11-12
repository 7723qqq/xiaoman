//! Argument parsing, environment merging and the console channels.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufReader, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use xiaoman_api::{Action, ApprovalGateway, ToolApprovalGateway, UserChannel};
use xiaoman_api::{
    ApprovalRequest, ChatRequest, ChatResponse, LlmClient, LlmError, ModelConfig,
    ToolApprovalRequest,
};
use xiaoman_ios::ios_platform::IosPlatform;
use xiaoman_ios::runner::assembly::LlmFactory;
use xiaoman_ios::runner::channels::{ConsoleChannels, NOBODY_HERE};
use xiaoman_ios::runner::command::Runner;
use xiaoman_ios::runner::config::{CliError, Command, DEFAULT_BASE_URL, RunnerConfig, USAGE};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct TempHome(PathBuf);

impl TempHome {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "xiaoman-ios-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.0.join(name), text).unwrap();
    }

    fn path_string(&self) -> String {
        self.0.to_string_lossy().to_string()
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        if self.0.starts_with(std::env::temp_dir()) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buffer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).to_string()
    }
}

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn make_runner(
    home: &Path,
    env: BTreeMap<String, String>,
    input: &str,
) -> (Runner, Buffer, Buffer) {
    let out = Buffer::default();
    let err = Buffer::default();
    let runner = Runner::new()
        .with_env(env)
        .with_input(Box::new(BufReader::new(Cursor::new(
            input.as_bytes().to_vec(),
        ))))
        .with_out(Box::new(out.clone()))
        .with_err(Box::new(err.clone()))
        .with_console(false)
        .with_user_home(home);
    (runner, out, err)
}

#[tokio::test]
async fn help_and_empty_usage_exit_codes() {
    let home = TempHome::new("usage");
    let (runner, out, err) = make_runner(
        home.path(),
        env(&[("XIAOMAN_HOME", &home.path_string())]),
        "",
    );
    assert_eq!(runner.run(args(&["--help"])).await, 0);
    assert!(out.text().contains("ios-runner check"), "{}", out.text());
    assert_eq!(err.text(), "");

    let (runner, _out, err) = make_runner(
        home.path(),
        env(&[("XIAOMAN_HOME", &home.path_string())]),
        "",
    );
    assert_eq!(runner.run(args(&[])).await, 2);
    assert!(err.text().contains(USAGE));

    let (runner, out, _err) = make_runner(
        home.path(),
        env(&[("XIAOMAN_HOME", &home.path_string())]),
        "",
    );
    assert_eq!(runner.run(args(&["check", "--help"])).await, 0);
    assert!(out.text().contains("ios-runner check"));
}

#[tokio::test]
async fn bad_usage_exits_two() {
    let home = TempHome::new("bad-usage");
    let env = env(&[("XIAOMAN_HOME", &home.path_string())]);
    for arguments in [
        vec!["fly"],
        vec!["task"],
        vec!["dump"],
        vec!["check", "--max-steps", "0"],
        vec!["check", "--wda"],
        vec!["check", "--bogus"],
    ] {
        let (runner, out, err) = make_runner(home.path(), env.clone(), "");
        let arguments: Vec<String> = arguments.into_iter().map(str::to_string).collect();
        assert_eq!(runner.run(arguments).await, 2, "{err:?}");
        assert!(err.text().contains("用法"), "{}", err.text());
        assert_eq!(out.text(), "");
    }
}

#[tokio::test]
async fn check_without_wda_gives_hint() {
    let home = TempHome::new("no-wda");
    let (runner, _out, err) = make_runner(
        home.path(),
        env(&[("XIAOMAN_HOME", &home.path_string())]),
        "",
    );
    let code = runner
        .run(args(&[
            "check",
            "--wda",
            "http://127.0.0.1:1",
            "--non-interactive",
        ]))
        .await;
    assert_eq!(code, 1);
    assert!(err.text().contains("wda.sh start"), "{}", err.text());
}

#[tokio::test]
async fn task_without_models_exits_two() {
    let home = TempHome::new("no-models");
    let (runner, _out, err) = make_runner(
        home.path(),
        env(&[("XIAOMAN_HOME", &home.path_string())]),
        "",
    );
    assert_eq!(runner.run(args(&["task", "随便"])).await, 2);
    assert!(err.text().contains("模型还没配置好"), "{}", err.text());
}

#[test]
fn merges_args_env_and_properties_without_leaking_keys() {
    let home = TempHome::new("merge");
    home.write(
        "config.properties",
        "chat.model=doubao-chat\noperator.model=file-operator\nchat.apiKey=sk-in-file\n",
    );
    let environment = env(&[
        ("XIAOMAN_HOME", &home.path_string()),
        ("XIAOMAN_OPERATOR_MODEL", "doubao-vision"),
        ("ARK_API_KEY", "ark-secret-123"),
        ("XIAOMAN_WDA_URL", "http://10.0.0.2:8100"),
    ]);
    let config = RunnerConfig::parse(
        &args(&[
            "task",
            "打开设置",
            "--skills",
            "/repo/skills/ios",
            "--skill",
            "open-settings-wifi",
            "--max-steps",
            "12",
            "--non-interactive",
        ]),
        &environment,
        home.path(),
    )
    .unwrap();
    assert_eq!(config.command, Command::Task);
    assert_eq!(config.goal.as_deref(), Some("打开设置"));
    assert_eq!(config.skill.as_deref(), Some("open-settings-wifi"));
    assert_eq!(config.max_steps, 12);
    assert!(config.non_interactive);
    assert_eq!(config.wda_url, "http://10.0.0.2:8100");
    assert_eq!(config.chat.model, "doubao-chat");
    assert_eq!(config.operator.model, "doubao-vision");
    assert_eq!(config.operator.base_url, DEFAULT_BASE_URL);
    assert_eq!(config.chat.api_key, "ark-secret-123");
    assert_eq!(config.operator.api_key, "ark-secret-123");
    assert_eq!(
        config.skill_dirs(),
        vec![
            PathBuf::from("/repo/skills/ios"),
            home.path().join("skills")
        ]
    );
    assert_eq!(config.warnings.len(), 1);
    assert!(config.warnings[0].contains("chat.apiKey"));
    assert_eq!(config.missing_models(), None);
    let debug = format!("{config:?}");
    assert!(!debug.contains("ark-secret-123"), "{debug}");
    assert!(!debug.contains("sk-in-file"), "{debug}");
}

#[test]
fn explicit_args_win_and_specific_keys_beat_shared_key() {
    let home = TempHome::new("override");
    let environment = env(&[
        ("XIAOMAN_HOME", &home.path_string()),
        ("XIAOMAN_WDA_URL", "http://env:8100"),
        ("ARK_API_KEY", "shared"),
        ("XIAOMAN_CHAT_API_KEY", "chat-only"),
    ]);
    let config = RunnerConfig::parse(
        &args(&["check", "--wda", "http://arg:8100"]),
        &environment,
        home.path(),
    )
    .unwrap();
    assert_eq!(config.wda_url, "http://arg:8100");
    assert_eq!(config.chat.api_key, "chat-only");
    assert_eq!(config.operator.api_key, "shared");
}

#[test]
fn wda_bundle_id_follows_the_script() {
    let home = TempHome::new("bundle");
    let default = RunnerConfig::parse(
        &args(&["check"]),
        &env(&[("XIAOMAN_HOME", &home.path_string())]),
        home.path(),
    )
    .unwrap();
    assert_eq!(
        IosPlatform::runner_bundle_id(default.wda_bundle_id.as_deref()),
        "com.xiaoman.WebDriverAgentRunner.xctrunner"
    );
    let custom = RunnerConfig::parse(
        &args(&["check"]),
        &env(&[
            ("XIAOMAN_HOME", &home.path_string()),
            ("WDA_BUNDLE_ID", "com.ryan.wda"),
        ]),
        home.path(),
    )
    .unwrap();
    assert_eq!(
        IosPlatform::runner_bundle_id(custom.wda_bundle_id.as_deref()),
        "com.ryan.wda.xctrunner"
    );
}

#[test]
fn missing_models_explains_without_keys() {
    let home = TempHome::new("missing-models");
    let config = RunnerConfig::parse(
        &args(&["chat", "--home", &home.path_string()]),
        &env(&[("XIAOMAN_CHAT_API_KEY", "only-chat-key")]),
        home.path(),
    )
    .unwrap();
    let message = config.missing_models().expect("missing models");
    assert!(message.contains("chat.model"), "{message}");
    assert!(message.contains("operator.model"), "{message}");
    assert!(message.contains("XIAOMAN_OPERATOR_API_KEY"), "{message}");
    assert!(!message.contains("only-chat-key"), "{message}");
}

#[test]
fn rejects_bad_usage() {
    let home = TempHome::new("reject");
    let environment = env(&[("XIAOMAN_HOME", &home.path_string())]);
    for arguments in [
        vec!["fly"],
        vec!["task"],
        vec!["dump"],
        vec!["check", "--max-steps", "0"],
        vec!["check", "--wda"],
        vec!["check", "--bogus"],
    ] {
        let arguments: Vec<String> = arguments.into_iter().map(str::to_string).collect();
        assert!(
            matches!(
                RunnerConfig::parse(&arguments, &environment, home.path()),
                Err(CliError::Usage(_))
            ),
            "{arguments:?}"
        );
    }
    assert!(matches!(
        RunnerConfig::parse(&args(&["--help"]), &environment, home.path()),
        Err(CliError::Help(_))
    ));
}

fn channels(input: &str, interactive: bool) -> (ConsoleChannels, Buffer) {
    let out = Buffer::default();
    let channels = ConsoleChannels::new(
        Box::new(BufReader::new(Cursor::new(input.as_bytes().to_vec()))),
        Box::new(out.clone()),
        interactive,
    );
    (channels, out)
}

fn approval_request() -> ApprovalRequest {
    ApprovalRequest {
        task_id: "t1".to_string(),
        action: Action::Tap {
            node_id: Some(3),
            point: None,
            target: Some("立即支付".to_string()),
            reason: None,
        },
        prompt: "点击「立即支付」".to_string(),
        reason: "确认词：支付".to_string(),
    }
}

#[tokio::test]
async fn interactive_answers_map_to_approvals() {
    let (channels, out) = channels("y\nt\nn\n\n", true);
    let request = approval_request();
    let first = channels.request(request.clone()).await.unwrap();
    assert!(first.approved);
    assert_eq!(first.scope, xiaoman_api::ApprovalScope::Once);
    let second = channels.request(request.clone()).await.unwrap();
    assert!(second.approved);
    assert_eq!(second.scope, xiaoman_api::ApprovalScope::Task);
    for _ in 0..2 {
        assert!(!channels.request(request.clone()).await.unwrap().approved);
    }
    assert!(!channels.request(request).await.unwrap().approved); // EOF
    assert!(
        out.text().contains("需要确认：点击「立即支付」"),
        "{}",
        out.text()
    );
}

#[tokio::test]
async fn non_interactive_denies_and_answers_fixed_text() {
    let (channels, out) = channels("y\n回答\n", false);
    assert!(!channels.request(approval_request()).await.unwrap().approved);
    let tool = ToolApprovalRequest {
        task_id: "t1".to_string(),
        server: "测试服务".to_string(),
        tool: "order".to_string(),
        title: None,
        arguments_json: "{}".to_string(),
        reason: "写操作".to_string(),
    };
    assert!(!channels.approve(tool).await);
    assert_eq!(
        channels.ask("t1", "要买哪个颜色？", false).await,
        NOBODY_HERE
    );
    assert!(out.text().contains("已自动拒绝"), "{}", out.text());
}

#[tokio::test]
async fn interactive_questions_read_one_line() {
    let (channels, out) = channels("黑色\n", true);
    assert_eq!(channels.ask("t1", "要买哪个颜色？", false).await, "黑色");
    assert_eq!(channels.ask("t1", "请完成验证码", true).await, NOBODY_HERE);
    assert!(
        out.text().contains("请在手机上完成这一步"),
        "{}",
        out.text()
    );
}

const SETTINGS_XML: &str = r#"<AppiumAUT><XCUIElementTypeApplication type="XCUIElementTypeApplication" visible="true" x="0" y="0" width="390" height="844">
  <XCUIElementTypeCell type="XCUIElementTypeCell" visible="true" enabled="true" label="无线局域网" x="0" y="200" width="390" height="44"/>
  <XCUIElementTypeStaticText type="XCUIElementTypeStaticText" visible="true" label="设置" x="16" y="100" width="100" height="40"/>
</XCUIElementTypeApplication></AppiumAUT>"#;

const PAY_XML: &str = r#"<AppiumAUT><XCUIElementTypeApplication type="XCUIElementTypeApplication" visible="true" x="0" y="0" width="390" height="844">
  <XCUIElementTypeStaticText type="XCUIElementTypeStaticText" visible="true" label="合计 ¥2.40" x="16" y="700" width="200" height="40"/>
  <XCUIElementTypeButton type="XCUIElementTypeButton" visible="true" enabled="true" label="立即支付" x="250" y="780" width="120" height="44"/>
</XCUIElementTypeApplication></AppiumAUT>"#;

const KEY: &str = "ark-test-key-9f8e7d";

fn screenshot() -> String {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::new(390, 844));
    let mut cursor = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
    base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        cursor.into_inner(),
    )
}

struct WdaState {
    xml: String,
    bundle_id: String,
    screenshot: String,
    paths: Vec<String>,
    actions: usize,
    launches: usize,
}

#[derive(Clone)]
struct WdaResponder(Arc<Mutex<WdaState>>);

impl Respond for WdaResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        let path = match request.url.query() {
            Some(query) => format!("{}?{}", request.url.path(), query),
            None => request.url.path().to_string(),
        };
        state.paths.push(format!("{} {path}", request.method));
        let (status, value) = if path == "/session" {
            (200, json!({ "sessionId": "s1" }))
        } else if path == "/status" {
            (
                200,
                json!({ "value": { "ready": true, "os": { "version": "26.1" } } }),
            )
        } else if path.ends_with("/window/size") {
            (200, json!({ "value": { "width": 390, "height": 844 } }))
        } else if path == "/screenshot" {
            (200, json!({ "value": state.screenshot.clone() }))
        } else if path.starts_with("/source") {
            (200, json!({ "value": state.xml.clone() }))
        } else if path == "/wda/activeAppInfo" {
            (
                200,
                json!({ "value": { "bundleId": state.bundle_id.clone() } }),
            )
        } else if path.ends_with("/wda/apps/launch") {
            state.launches += 1;
            state.bundle_id = "com.apple.Preferences".to_string();
            (200, json!({ "value": null }))
        } else if path.ends_with("/elements") {
            (200, json!({ "value": [] }))
        } else {
            if path.ends_with("/actions") {
                state.actions += 1;
            }
            (200, json!({ "value": null }))
        };
        ResponseTemplate::new(status).set_body_json(value)
    }
}

struct SmokeWda {
    server: MockServer,
    state: Arc<Mutex<WdaState>>,
}

impl SmokeWda {
    async fn start(xml: &str, bundle_id: &str) -> Self {
        let state = Arc::new(Mutex::new(WdaState {
            xml: xml.to_string(),
            bundle_id: bundle_id.to_string(),
            screenshot: screenshot(),
            paths: Vec::new(),
            actions: 0,
            launches: 0,
        }));
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(WdaResponder(state.clone()))
            .mount(&server)
            .await;
        Self { server, state }
    }

    fn uri(&self) -> String {
        self.server.uri()
    }

    fn count(&self, suffix: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .paths
            .iter()
            .filter(|path| path.ends_with(suffix))
            .count()
    }

    fn actions(&self) -> usize {
        self.state.lock().unwrap().actions
    }

    fn launches(&self) -> usize {
        self.state.lock().unwrap().launches
    }

    fn paths(&self) -> Vec<String> {
        self.state.lock().unwrap().paths.clone()
    }
}

struct ScriptedLlm {
    config: ModelConfig,
    replies: Arc<Mutex<VecDeque<String>>>,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    fn config(&self) -> &ModelConfig {
        &self.config
    }

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.requests.lock().unwrap().push(request);
        let text = self.replies.lock().unwrap().pop_front().unwrap_or_else(|| {
            r#"{"thought":"脚本结束","action":{"type":"finish","success":false,"summary":"脚本用完"}}"#.to_string()
        });
        Ok(ChatResponse {
            text,
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_string()),
            usage: None,
        })
    }
}

fn scripted_llm(replies: Vec<&str>) -> (LlmFactory, Arc<Mutex<Vec<ChatRequest>>>) {
    let queue: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(
        replies.into_iter().map(str::to_string).collect(),
    ));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let factory: LlmFactory = {
        let queue = queue.clone();
        let requests = requests.clone();
        Arc::new(move |config| {
            Arc::new(ScriptedLlm {
                config,
                replies: queue.clone(),
                requests: requests.clone(),
            }) as Arc<dyn LlmClient>
        })
    };
    (factory, requests)
}

#[tokio::test]
async fn check_reports_screen() {
    let home = TempHome::new("check-screen");
    let wda = SmokeWda::start(SETTINGS_XML, "com.apple.springboard").await;
    let url = wda.uri();
    let environment = env(&[
        ("XIAOMAN_HOME", &home.path_string()),
        ("XIAOMAN_WDA_URL", &url),
    ]);
    let (runner, out, err) = make_runner(home.path(), environment, "");
    assert_eq!(runner.run(args(&["check"])).await, 0, "{}", err.text());
    let text = out.text();
    assert!(text.contains("390x844 点"), "{text}");
    assert!(text.contains("com.apple.springboard"), "{text}");
    assert!(text.contains("iOS 26.1"), "{text}");
    assert!(text.contains("节点：2 个"), "{text}");
    assert!(text.contains("连接正常。"), "{text}");
}

#[tokio::test]
async fn dump_saves_screenshot_source_and_nodes() {
    let home = TempHome::new("dump");
    let wda = SmokeWda::start(SETTINGS_XML, "com.apple.springboard").await;
    let url = wda.uri();
    let environment = env(&[
        ("XIAOMAN_HOME", &home.path_string()),
        ("XIAOMAN_WDA_URL", &url),
    ]);
    let (runner, out, err) = make_runner(home.path(), environment, "");
    let dir = home.path().join("dump");
    let dir_text = dir.to_string_lossy().to_string();
    assert_eq!(
        runner.run(args(&["dump", "--out", &dir_text])).await,
        0,
        "{}",
        err.text()
    );
    assert!(dir.join("screenshot.png").is_file());
    assert!(dir.join("source.xml").is_file());
    let nodes = std::fs::read_to_string(dir.join("nodes.txt")).unwrap();
    assert!(nodes.contains("无线局域网"), "{nodes}");
    assert!(out.text().contains("已保存到"), "{}", out.text());
}

#[tokio::test]
async fn task_launches_taps_and_finishes_with_trace() {
    let home = TempHome::new("task");
    let wda = SmokeWda::start(SETTINGS_XML, "com.apple.springboard").await;
    let url = wda.uri();
    let environment = env(&[
        ("XIAOMAN_HOME", &home.path_string()),
        ("XIAOMAN_WDA_URL", &url),
        ("ARK_API_KEY", KEY),
        ("XIAOMAN_CHAT_MODEL", "chat-m"),
        ("XIAOMAN_OPERATOR_MODEL", "op-m"),
    ]);
    let (runner, out, err) = make_runner(home.path(), environment, "");
    let (llm, _requests) = scripted_llm(vec![
        r#"{"thought":"先打开设置","action":{"type":"launch","packageName":"com.apple.Preferences"}}"#,
        r#"{"thought":"点无线局域网","action":{"type":"tap","nodeId":0,"target":"无线局域网"}}"#,
        r#"{"thought":"完成","action":{"type":"finish","success":true,"summary":"无线局域网已打开，连接 Home"}}"#,
    ]);
    let code = runner
        .with_llm(llm)
        .run(args(&[
            "task",
            "打开设置里的无线局域网页面",
            "--non-interactive",
        ]))
        .await;
    let text = out.text();
    assert_eq!(code, 0, "{text}\n{}", err.text());
    assert_eq!(wda.launches(), 1, "{:?}", wda.paths());
    assert_eq!(wda.actions(), 1, "{:?}", wda.paths());
    assert_eq!(wda.count("/wda/apps/launch"), 1, "{:?}", wda.paths());
    assert!(text.contains("任务完成"), "{text}");

    let trace_dirs: Vec<_> = std::fs::read_dir(home.path().join("traces"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(trace_dirs.len(), 1, "{trace_dirs:?}");
    let trace = &trace_dirs[0];
    let events = std::fs::read_to_string(trace.join("events.jsonl")).unwrap();
    assert!(events.contains("\"perform\""), "{events}");
    assert!(events.contains("LaunchApp"), "{events}");
    assert!(!events.contains(KEY), "{events}");
    for line in events.lines().filter(|line| !line.trim().is_empty()) {
        serde_json::from_str::<Value>(line).unwrap();
    }
    let steps: Vec<_> = std::fs::read_dir(trace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with("step-") && name.ends_with(".png"))
        .collect();
    assert!(!steps.is_empty(), "no step screenshots in {trace:?}");
}

#[tokio::test]
async fn payment_tap_is_blocked_when_nobody_is_there() {
    let home = TempHome::new("payment");
    let wda = SmokeWda::start(PAY_XML, "com.taobao.taobao4iphone").await;
    let url = wda.uri();
    let environment = env(&[
        ("XIAOMAN_HOME", &home.path_string()),
        ("XIAOMAN_WDA_URL", &url),
        ("ARK_API_KEY", KEY),
        ("XIAOMAN_CHAT_MODEL", "chat-m"),
        ("XIAOMAN_OPERATOR_MODEL", "op-m"),
    ]);
    let (runner, out, err) = make_runner(home.path(), environment, "");
    let (llm, _requests) = scripted_llm(vec![
        r#"{"thought":"付款","action":{"type":"tap","nodeId":1,"target":"立即支付 按钮"}}"#,
        r#"{"thought":"无法付款","action":{"type":"finish","success":false,"summary":"停在支付前"}}"#,
    ]);
    let code = runner
        .with_llm(llm)
        .run(args(&["task", "帮我付款", "--non-interactive"]))
        .await;
    let text = out.text();
    assert_eq!(code, 1, "{text}\n{}", err.text());
    assert_eq!(wda.actions(), 0, "{:?}", wda.paths());
    assert!(text.contains("已自动拒绝"), "{text}");
    assert!(text.contains("被安全模块拦下"), "{text}");
}
