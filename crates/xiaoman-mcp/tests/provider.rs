use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use xiaoman_api::{ToolApprovalGateway, ToolApprovalRequest, ToolProvider, UserChannel};
use xiaoman_mcp::{
    ConfirmPolicy, McpConfig, McpError, McpServerConfig, McpToolProvider, McpTransport,
    UserChannelApprovals,
};

/// Stands in for one server: answers `tools/list` and `tools/call`, records every message.
struct FakeTransport {
    tools: Vec<Value>,
    result: Mutex<Value>,
    messages: Mutex<Vec<Value>>,
    failure: Mutex<Option<String>>,
    closed: AtomicBool,
}

impl FakeTransport {
    fn new(tools: Vec<Value>) -> Arc<Self> {
        Self::with_result(
            tools,
            json!({"content": [{"type": "text", "text": "done"}]}),
        )
    }

    fn with_result(tools: Vec<Value>, result: Value) -> Arc<Self> {
        Arc::new(Self {
            tools,
            result: Mutex::new(result),
            messages: Mutex::new(Vec::new()),
            failure: Mutex::new(None),
            closed: AtomicBool::new(false),
        })
    }

    fn fail(&self, message: &str) {
        *self.failure.lock().unwrap() = Some(message.to_string());
    }

    fn messages(&self) -> Vec<Value> {
        self.messages.lock().unwrap().clone()
    }

    fn count(&self, method: &str) -> usize {
        self.messages
            .lock()
            .unwrap()
            .iter()
            .filter(|message| message["method"] == method)
            .count()
    }

    fn call_count(&self) -> usize {
        self.count("tools/call")
    }

    fn last_call_arguments(&self) -> Value {
        self.messages()
            .iter()
            .rev()
            .find(|message| message["method"] == "tools/call")
            .map(|message| message["params"]["arguments"].clone())
            .unwrap_or(Value::Null)
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl McpTransport for FakeTransport {
    async fn exchange(&self, message: Value) -> Result<Option<Value>, McpError> {
        self.messages.lock().unwrap().push(message.clone());
        if message["method"] == "tools/call" {
            if let Some(failure) = self.failure.lock().unwrap().clone() {
                return Err(McpError(failure));
            }
        }
        let Some(id) = message.get("id").cloned() else {
            return Ok(None);
        };
        let response = match message["method"].as_str().unwrap_or_default() {
            "tools/list" => json!({ "tools": &self.tools }),
            "tools/call" => self.result.lock().unwrap().clone(),
            _ => json!({}),
        };
        Ok(Some(
            json!({ "jsonrpc": "2.0", "id": id, "result": response }),
        ))
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

struct Approvals {
    reply: bool,
    requests: Mutex<Vec<ToolApprovalRequest>>,
}

impl Approvals {
    fn new(reply: bool) -> Arc<Self> {
        Arc::new(Self {
            reply,
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ToolApprovalRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ToolApprovalGateway for Approvals {
    async fn approve(&self, request: ToolApprovalRequest) -> bool {
        self.requests.lock().unwrap().push(request);
        self.reply
    }
}

fn tool(name: &str) -> Value {
    json!({"name": name, "inputSchema": {"type": "object", "properties": {}}})
}

fn tool_with(
    name: &str,
    read_only: Option<bool>,
    title: Option<&str>,
    description: Option<&str>,
) -> Value {
    let mut tool = tool(name);
    if let Some(read_only) = read_only {
        tool["annotations"] = json!({"readOnlyHint": read_only});
    }
    if let Some(title) = title {
        tool["title"] = json!(title);
    }
    if let Some(description) = description {
        tool["description"] = json!(description);
    }
    tool
}

fn serve(
    transport: Arc<FakeTransport>,
) -> impl Fn(&McpServerConfig) -> Result<Arc<dyn McpTransport>, McpError> + Send + Sync + 'static {
    move |_| Ok(transport.clone())
}

fn config(servers: Vec<McpServerConfig>) -> McpConfig {
    McpConfig {
        servers,
        warnings: Vec::new(),
    }
}

fn provider(
    config: McpConfig,
    approvals: Arc<Approvals>,
    transport: Arc<FakeTransport>,
) -> McpToolProvider {
    McpToolProvider::new(config, approvals).with_transport_factory(serve(transport))
}

#[tokio::test]
async fn names_are_sanitized_bounded_unique_and_stable() {
    let fake = FakeTransport::new(vec![
        tool("normal"),
        tool("a.b"),
        tool("a/b"),
        tool(&"x".repeat(100)),
        tool_with("重复", None, Some("查询"), Some("天气")),
    ]);
    let mut maps = McpServerConfig::new("maps");
    maps.title = "地图".to_string();
    let provider = provider(config(vec![maps]), Approvals::new(true), fake.clone());

    assert!(fake.messages().is_empty());
    let specs = provider.specs().await;
    assert_eq!(specs[0].name, "maps__normal");
    assert_eq!(specs[1].name, "maps__a_b");
    assert_eq!(specs[2].name, "maps__a_b_2");
    assert_eq!(specs[3].name.chars().count(), 64);
    assert_eq!(
        specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        5
    );
    assert!(specs.iter().all(|spec| {
        let length = spec.name.chars().count();
        (1..=64).contains(&length)
            && spec.name.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '_' || character == '-'
            })
    }));
    assert!(
        specs
            .last()
            .unwrap()
            .description
            .starts_with("[服务器 地图] 查询：天气")
    );

    assert_eq!(specs, provider.specs().await);
    assert_eq!(fake.count("initialize"), 1);
    assert!(provider.call("task", "unknown", "{}").await.is_none());

    let dynamic: &dyn ToolProvider = &provider;
    assert_eq!(dynamic.specs().await.len(), 5);
    provider.close();
    assert!(fake.closed());
}

#[tokio::test]
async fn writes_approves_only_non_read_only_tools() {
    let fake = FakeTransport::new(vec![
        tool_with("read", Some(true), None, None),
        tool_with("write", Some(false), None, None),
        tool("unspecified"),
    ]);
    let approvals = Approvals::new(true);
    let provider = provider(
        config(vec![McpServerConfig::new("maps")]),
        approvals.clone(),
        fake.clone(),
    );

    provider.specs().await;
    assert!(provider.call("task", "maps__read", "").await.unwrap().ok);
    assert!(provider.call("task", "maps__write", "{}").await.unwrap().ok);
    assert!(
        provider
            .call("task", "maps__unspecified", "{}")
            .await
            .unwrap()
            .ok
    );

    let requests = approvals.requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.tool.as_str())
            .collect::<Vec<_>>(),
        vec!["write", "unspecified"]
    );
    assert_eq!(requests[0].task_id, "task");
    assert!(requests[0].reason.contains("修改外部数据"));
    assert_eq!(fake.call_count(), 3);
    assert_eq!(fake.last_call_arguments(), json!({}));
}

#[tokio::test]
async fn all_confirms_even_read_only_and_none_never_confirms() {
    for (policy, expected) in [(ConfirmPolicy::All, 1), (ConfirmPolicy::None, 0)] {
        let fake = FakeTransport::new(vec![tool_with("weather", Some(true), None, None)]);
        let mut maps = McpServerConfig::new("maps");
        maps.confirm = policy;
        let approvals = Approvals::new(true);
        let provider = provider(config(vec![maps]), approvals.clone(), fake.clone());

        provider.specs().await;
        assert!(
            provider
                .call("task", "maps__weather", "{}")
                .await
                .unwrap()
                .ok
        );
        assert_eq!(approvals.requests().len(), expected);
        assert_eq!(fake.call_count(), 1);
    }
}

#[tokio::test]
async fn rejection_does_not_call_the_server() {
    let fake = FakeTransport::new(vec![tool("weather")]);
    let provider = provider(
        config(vec![McpServerConfig::new("maps")]),
        Approvals::new(false),
        fake.clone(),
    );

    provider.specs().await;
    let result = provider.call("task", "maps__weather", "{}").await.unwrap();
    assert!(!result.ok);
    assert!(result.text.contains("用户没有确认"));
    assert_eq!(fake.call_count(), 0);
}

#[tokio::test]
async fn error_results_and_transport_exceptions_return_false() {
    let fake = FakeTransport::with_result(
        vec![tool("weather")],
        json!({"isError": true, "content": [{"type": "text", "text": "server error"}]}),
    );
    let mut maps = McpServerConfig::new("maps");
    maps.confirm = ConfirmPolicy::None;
    let provider = provider(config(vec![maps]), Approvals::new(true), fake.clone());

    provider.specs().await;
    assert!(
        !provider
            .call("task", "maps__weather", "{}")
            .await
            .unwrap()
            .ok
    );
    fake.fail("connection failed");
    let result = provider.call("task", "maps__weather", "{}").await.unwrap();
    assert!(!result.ok);
    assert!(result.text.contains("connection failed"));
}

#[tokio::test]
async fn bad_arguments_and_rejected_approval_return_errors_without_a_call() {
    let fake = FakeTransport::new(vec![tool("weather")]);
    // The Rust gateway has no error channel: an unavailable gateway answers "no".
    let provider = provider(
        config(vec![McpServerConfig::new("maps")]),
        Approvals::new(false),
        fake.clone(),
    );

    provider.specs().await;
    for arguments in ["[]", "not-json", "{}"] {
        assert!(
            !provider
                .call("task", "maps__weather", arguments)
                .await
                .unwrap()
                .ok
        );
    }
    assert_eq!(fake.call_count(), 0);
}

#[tokio::test]
async fn long_results_are_truncated_after_redaction() {
    let fake = FakeTransport::with_result(
        vec![tool("weather")],
        json!({"content": [{"type": "text", "text": format!("url-secret{}", "x".repeat(9000))}]}),
    );
    let mut maps = McpServerConfig::new("maps");
    maps.url = Some("https://example.test?key=url-secret".to_string());
    maps.confirm = ConfirmPolicy::None;
    let provider =
        provider(config(vec![maps]), Approvals::new(true), fake.clone()).with_max_result_chars(100);

    provider.specs().await;
    let result = provider.call("task", "maps__weather", "{}").await.unwrap();
    assert_eq!(result.text.chars().count(), 100);
    assert!(result.text.ends_with("[结果已截断]"));
    assert!(!result.text.contains("url-secret"));
}

#[tokio::test]
async fn every_model_field_approval_and_status_redacts_all_secrets() {
    let secret_text = "url+secret url%2Bsecret header-secret token-secret env-secret";
    let mut maps = McpServerConfig::new("maps");
    maps.title = secret_text.to_string();
    maps.url = Some("https://example.test?key=url%2Bsecret".to_string());
    maps.headers
        .insert("X-Key".to_string(), "header-secret".to_string());
    maps.headers.insert(
        "Authorization".to_string(),
        "Bearer token-secret".to_string(),
    );
    maps.env.insert("KEY".to_string(), "env-secret".to_string());

    let raw_tool = json!({
        "name": "weather",
        "title": secret_text,
        "description": secret_text,
        "inputSchema": {"type": "object", "description": secret_text},
    });
    let fake = FakeTransport::with_result(
        vec![raw_tool],
        json!({"content": [{"type": "text", "text": secret_text}]}),
    );
    let approvals = Approvals::new(true);
    let provider = provider(config(vec![maps.clone()]), approvals.clone(), fake.clone());

    let specs = provider.specs().await;
    assert_eq!(specs.len(), 1);
    let success = provider
        .call(
            "task",
            &specs[0].name,
            &json!({ "value": secret_text }).to_string(),
        )
        .await
        .unwrap();
    fake.fail(secret_text);
    let failure = provider.call("task", &specs[0].name, "{}").await.unwrap();
    let statuses = provider.status(false).await;

    let visible = format!(
        "{specs:?}\n{success:?}\n{failure:?}\n{:?}\n{statuses:?}\n{maps:?}",
        approvals.requests()
    );
    for secret in secret_text.split(' ') {
        assert!(!visible.contains(secret), "leaked {secret}: {visible}");
    }
    assert!(success.ok);
    assert!(!failure.ok);
}

#[tokio::test]
async fn server_failures_are_isolated_and_retried_with_stable_names() {
    let good = FakeTransport::new(vec![tool("a/b")]);
    let bad = FakeTransport::new(vec![tool("a.b")]);
    let attempts = Arc::new(AtomicUsize::new(0));

    let mut good_config = McpServerConfig::new("good");
    good_config.confirm = ConfirmPolicy::None;
    let mut bad_config = McpServerConfig::new("bad");
    bad_config.url = Some("https://example.test?key=secret-key".to_string());
    bad_config
        .headers
        .insert("X".to_string(), "header-secret".to_string());
    let mut disabled = McpServerConfig::new("disabled");
    disabled.enabled = false;

    let good_fake = good.clone();
    let bad_fake = bad.clone();
    let counter = attempts.clone();
    let factory = move |server: &McpServerConfig| -> Result<Arc<dyn McpTransport>, McpError> {
        if server.name == "good" {
            return Ok(good_fake.clone());
        }
        let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt <= 2 {
            return Err(McpError("offline secret-key header-secret".to_string()));
        }
        Ok(bad_fake.clone())
    };
    let provider = McpToolProvider::new(
        config(vec![good_config, bad_config, disabled]),
        Approvals::new(true),
    )
    .with_transport_factory(factory)
    .with_retry_after_millis(0);

    let initial = provider.specs().await;
    assert_eq!(initial.len(), 1);
    assert!(
        provider
            .call("task", &initial[0].name, "{}")
            .await
            .unwrap()
            .ok
    );

    let statuses = provider.status(false).await;
    assert!(statuses[0].connected);
    assert_eq!(statuses[0].tool_count, 1);
    assert!(!statuses[1].connected);
    assert!(
        statuses[1]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("offline")
    );
    let status_text = format!("{statuses:?}");
    assert!(!status_text.contains("secret-key"));
    assert!(!status_text.contains("header-secret"));
    assert!(!statuses[2].connected);

    let recovered = provider.specs().await;
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered[0], initial[0]);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(good.count("initialize"), 1);
}

#[tokio::test]
async fn whitelist_and_missing_schema_are_handled() {
    let fake = FakeTransport::new(vec![
        json!({"name": "keep", "inputSchema": "invalid"}),
        tool("skip"),
    ]);
    let mut maps = McpServerConfig::new("maps");
    maps.tools = Some(["keep".to_string()].into_iter().collect::<BTreeSet<_>>());
    let provider = provider(config(vec![maps]), Approvals::new(true), fake.clone());

    let specs = provider.specs().await;
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "maps__keep");
    assert_eq!(
        specs[0].parameters,
        json!({"type": "object", "properties": {}})
    );
}

#[tokio::test]
async fn structured_and_non_text_results_are_represented() {
    let fake = FakeTransport::with_result(
        vec![tool("weather")],
        json!({
            "content": [{"type": "image", "data": "binary"}, {"type": "resource"}],
            "structuredContent": {"count": 3},
        }),
    );
    let mut maps = McpServerConfig::new("maps");
    maps.confirm = ConfirmPolicy::None;
    let provider = provider(config(vec![maps]), Approvals::new(true), fake.clone());

    provider.specs().await;
    let result = provider.call("task", "maps__weather", "{}").await.unwrap();
    assert!(result.text.contains("[图片内容]"));
    assert!(result.text.contains("[资源内容]"));
    assert!(result.text.contains("\"count\":3"));
    assert!(!result.text.contains("binary"));
}

struct LoopingTransport {
    closed: AtomicBool,
}

#[async_trait]
impl McpTransport for LoopingTransport {
    async fn exchange(&self, message: Value) -> Result<Option<Value>, McpError> {
        let Some(id) = message.get("id").cloned() else {
            return Ok(None);
        };
        let result = if message["method"] == "tools/list" {
            json!({"tools": [], "nextCursor": "loop"})
        } else {
            json!({})
        };
        Ok(Some(json!({"id": id, "result": result})))
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn repeated_pagination_cursor_fails_and_closes_transport() {
    let transport = Arc::new(LoopingTransport {
        closed: AtomicBool::new(false),
    });
    let shared = transport.clone();
    let provider = McpToolProvider::new(
        config(vec![McpServerConfig::new("maps")]),
        Approvals::new(true),
    )
    .with_transport_factory(
        move |_: &McpServerConfig| -> Result<Arc<dyn McpTransport>, McpError> {
            Ok(shared.clone())
        },
    );

    assert!(provider.specs().await.is_empty());
    assert!(transport.closed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn short_config_values_do_not_corrupt_model_results() {
    let config = McpConfig::parse(
        r#"{"mcpServers":{"maps":{
            "url":"https://example.test?v=1&key=long-secret","env":{"NODE_ENV":"1"},"confirm":"none"}}}"#,
        &BTreeMap::new(),
    );
    let fake = FakeTransport::with_result(
        vec![tool_with("weather1", None, None, Some("温度 18"))],
        json!({"content": [{"type": "text", "text": "温度 18，key=long-secret"}]}),
    );
    let provider = provider(config, Approvals::new(true), fake.clone());

    let specs = provider.specs().await;
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].name, "maps__weather1");
    assert!(specs[0].description.contains("温度 18"));
    let result = provider.call("task", &specs[0].name, "{}").await.unwrap();
    assert_eq!(result.text, "温度 18，key=***");
    assert!(result.summary.contains("weather1"));
}

#[tokio::test]
async fn failed_server_waits_five_minutes_by_default_and_resets_the_delay() {
    let now = Arc::new(AtomicI64::new(0));
    let attempts = Arc::new(AtomicUsize::new(0));
    let good = FakeTransport::new(vec![tool("weather")]);

    let good_fake = good.clone();
    let counter = attempts.clone();
    let elapsed = now.clone();
    let factory = move |server: &McpServerConfig| -> Result<Arc<dyn McpTransport>, McpError> {
        if server.name == "good" {
            return Ok(good_fake.clone());
        }
        counter.fetch_add(1, Ordering::SeqCst);
        // Time spent inside the failing request: the interval starts at failure completion.
        elapsed.fetch_add(30_000, Ordering::SeqCst);
        Err(McpError("offline".to_string()))
    };
    let clock = now.clone();
    let provider = McpToolProvider::new(
        config(vec![
            McpServerConfig::new("good"),
            McpServerConfig::new("bad"),
        ]),
        Approvals::new(true),
    )
    .with_transport_factory(factory)
    .with_clock(move || clock.load(Ordering::SeqCst));

    assert_eq!(provider.specs().await.len(), 1);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    for _ in 0..3 {
        assert_eq!(provider.specs().await.len(), 1);
    }
    assert!(!provider.status(false).await.last().unwrap().connected);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    now.fetch_add(299_999, Ordering::SeqCst);
    provider.specs().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    now.fetch_add(1, Ordering::SeqCst);
    provider.specs().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    now.fetch_add(299_999, Ordering::SeqCst);
    provider.specs().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    now.fetch_add(1, Ordering::SeqCst);
    provider.specs().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(good.count("initialize"), 1);
}

#[tokio::test]
async fn status_refresh_bypasses_retry_delay_and_recovery_stays_cached() {
    let now = Arc::new(AtomicI64::new(10));
    let attempts = Arc::new(AtomicUsize::new(0));
    let fake = FakeTransport::new(vec![tool("weather")]);

    let fake_for_factory = fake.clone();
    let counter = attempts.clone();
    let factory = move |_: &McpServerConfig| -> Result<Arc<dyn McpTransport>, McpError> {
        if counter.fetch_add(1, Ordering::SeqCst) < 2 {
            return Err(McpError("offline".to_string()));
        }
        Ok(fake_for_factory.clone())
    };
    let clock = now.clone();
    let provider = McpToolProvider::new(
        config(vec![McpServerConfig::new("maps")]),
        Approvals::new(true),
    )
    .with_transport_factory(factory)
    .with_retry_after_millis(1000)
    .with_clock(move || clock.load(Ordering::SeqCst));

    assert!(provider.specs().await.is_empty());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(!provider.status(false).await[0].connected);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(!provider.status(true).await[0].connected);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    now.fetch_add(999, Ordering::SeqCst);
    assert!(provider.specs().await.is_empty());
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    let recovered = provider.status(true).await;
    assert_eq!(recovered.len(), 1);
    assert!(recovered[0].connected);
    assert_eq!(recovered[0].tool_count, 1);
    assert_eq!(recovered[0].error, None);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);

    now.fetch_add(1000, Ordering::SeqCst);
    assert_eq!(provider.specs().await.len(), 1);
    assert!(provider.status(true).await[0].connected);
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
}

struct Channel {
    reply: String,
    questions: Mutex<Vec<(String, String, bool)>>,
}

#[async_trait]
impl UserChannel for Channel {
    async fn ask(&self, task_id: &str, question: &str, takeover: bool) -> String {
        self.questions
            .lock()
            .unwrap()
            .push((task_id.to_string(), question.to_string(), takeover));
        self.reply.clone()
    }
}

#[tokio::test]
async fn user_channel_accepts_only_explicit_affirmatives() {
    let request = ToolApprovalRequest {
        task_id: "task".to_string(),
        server: "maps".to_string(),
        tool: "weather".to_string(),
        title: Some("天气".to_string()),
        arguments_json: "x".repeat(2000),
        reason: "需要确认".to_string(),
    };
    for reply in [
        "确认",
        " 是 ",
        "好",
        "可以",
        "Y",
        " YES ",
        "no",
        "确认一下",
        "",
    ] {
        let channel = Arc::new(Channel {
            reply: reply.to_string(),
            questions: Mutex::new(Vec::new()),
        });
        let gateway = UserChannelApprovals::new(channel.clone());
        let expected = matches!(
            reply.trim().to_lowercase().as_str(),
            "确认" | "是" | "好" | "可以" | "y" | "yes"
        );
        assert_eq!(gateway.approve(request.clone()).await, expected, "{reply}");

        let questions = channel.questions.lock().unwrap().clone();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].0, "task");
        assert!(!questions[0].2);
        assert!(
            questions[0].1.contains("「天气」（maps）"),
            "{}",
            questions[0].1
        );
        assert!(
            questions[0].1.contains("[参数已截断]"),
            "{}",
            questions[0].1
        );
        let length = questions[0].1.chars().count();
        assert!(length < 1200, "{length}");
    }
}
