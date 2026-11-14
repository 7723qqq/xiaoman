//! End-to-end tests over a real socket: the SSE chat stream, a static asset, and the health and
//! unknown-route contract of the Kotlin `web-server`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use xiaoman_api::{
    ChatRequest, ChatResponse, ChatRole, LlmClient, LlmError, ModelConfig, ToolCall,
};
use xiaoman_mcp::{McpError, McpServerConfig, McpTransport};
use xiaoman_web::{BoundServer, LlmFactory, WebOptions, WebServer};

const API_KEY: &str = "llm-secret-123";

/// A model that answers with a fixed reply, or asks for one of the built-in tools.
struct ScriptedModel {
    config: ModelConfig,
}

#[async_trait]
impl LlmClient for ScriptedModel {
    fn config(&self) -> &ModelConfig {
        &self.config
    }

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let last = request.messages.last().map(|message| message.role);
        if last == Some(ChatRole::Tool) {
            return Ok(reply("完成"));
        }
        let message = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == ChatRole::User)
            .map(|message| message.text())
            .unwrap_or_default();
        if message == "slow" {
            tokio::time::sleep(Duration::from_millis(300)).await;
            return Ok(reply("慢回复"));
        }
        let call = match message.as_str() {
            "reminder" => Some(ToolCall {
                id: "1".to_string(),
                name: "reminder_set".to_string(),
                arguments_json: r#"{"text":"喝水,带伞","at":"2099-11-03T18:30"}"#.to_string(),
            }),
            "memory" => Some(ToolCall {
                id: "1".to_string(),
                name: "memory_write".to_string(),
                arguments_json:
                    r#"{"title":"家","observations":[{"category":"地址","content":"北京"}]}"#
                        .to_string(),
            }),
            "mcp" => request
                .tools
                .iter()
                .find(|tool| tool.name.ends_with("__read"))
                .map(|tool| ToolCall {
                    id: "1".to_string(),
                    name: tool.name.clone(),
                    arguments_json: r#"{"value":"llm-secret-123 mcp-secret-456"}"#.to_string(),
                }),
            _ => None,
        };
        match call {
            Some(call) => Ok(ChatResponse {
                text: "开始".to_string(),
                tool_calls: vec![call],
                finish_reason: None,
                usage: None,
            }),
            None => Ok(reply("你好，我是小满")),
        }
    }
}

fn reply(text: &str) -> ChatResponse {
    ChatResponse {
        text: text.to_string(),
        tool_calls: Vec::new(),
        finish_reason: Some("stop".to_string()),
        usage: None,
    }
}

fn scripted() -> LlmFactory {
    Arc::new(|config: &ModelConfig| {
        Arc::new(ScriptedModel {
            config: config.clone(),
        }) as Arc<dyn LlmClient>
    })
}

struct TestServer {
    base: String,
    client: reqwest::Client,
    /// Kept alive for the duration of the test; dropping it stops the listener.
    _server: BoundServer,
}

impl TestServer {
    async fn start(options: WebOptions) -> Self {
        // The fake model lives at an unresolvable name, so the private-address check is off.
        let options = WebOptions {
            port: 0,
            allow_private: true,
            ..options
        };
        let server = WebServer::new(options)
            .with_llm(scripted())
            .with_logger(Arc::new(|_line: &str| {}))
            .bind()
            .await
            .expect("the server binds on an ephemeral port");
        let base = format!("http://127.0.0.1:{}", server.port());
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("client");
        Self {
            base,
            client,
            _server: server,
        }
    }

    fn port(&self) -> u16 {
        self._server.port()
    }

    async fn new_session(&self) -> String {
        let response = self
            .client
            .post(format!("{}/api/session", self.base))
            .json(&json!({}))
            .send()
            .await
            .expect("session request");
        let status = response.status();
        let body = response.text().await.expect("session body");
        assert_eq!(status, 200, "body: {body}");
        let body: Value = serde_json::from_str(&body).expect("session json");
        let id = body["sessionId"].as_str().expect("sessionId").to_string();
        assert_eq!(id.len(), 22);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        id
    }

    /// Sends one chat turn and returns the HTTP status and the decoded SSE events.
    async fn chat(&self, session: &str, body: &Value) -> (reqwest::StatusCode, Vec<Value>) {
        let response = self
            .client
            .post(format!("{}/api/chat", self.base))
            .header("x-xiaoman-session", session)
            .json(body)
            .send()
            .await
            .expect("chat request");
        let status = response.status();
        let text = tokio::time::timeout(Duration::from_secs(10), response.text())
            .await
            .expect("the stream ends")
            .expect("chat body");
        (status, events(&text, status))
    }
}

fn events(text: &str, status: reqwest::StatusCode) -> Vec<Value> {
    if status != 200 {
        return Vec::new();
    }
    text.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str::<Value>(data).expect("an SSE data line is JSON"))
        .collect()
}

fn chat_body(message: &str) -> Value {
    json!({
        "message": message,
        "model": { "baseUrl": "https://llm.invalid", "model": "fake", "apiKey": API_KEY },
        "mcp": { "keys": {}, "custom": [] },
    })
}

fn types(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .map(|event| event["type"].as_str().unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn vocab_route_returns_and_reuses_the_daily_plan() {
    let server = TestServer::start(WebOptions::default()).await;
    let session = server.new_session().await;
    let request = |session: &str| {
        server
            .client
            .post(format!("{}/api/vocab", server.base))
            .header("x-xiaoman-session", session)
            .json(&json!({"now": 1_763_000_000_000i64, "offsetMinutes": 480}))
    };

    let response = request(&session).send().await.expect("vocab request");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("vocab json");
    assert_eq!(body["new_words"].as_array().expect("new words").len(), 15);
    assert_eq!(body["stats"]["learned"], 15);
    assert_eq!(body["due_total"], 0);
    assert!(body["new_words"][0]["word"].is_string());
    assert!(body["new_words"][0]["meaning"].is_string());
    assert_eq!(body["prefs"]["daily_new"], 15);

    let response = request(&session)
        .send()
        .await
        .expect("second vocab request");
    let again: Value = response.json().await.expect("second vocab json");
    assert_eq!(again["new_words"], body["new_words"]);
    assert_eq!(again["stats"]["learned"], 15);
}

#[tokio::test]
async fn chat_streams_the_model_reply_and_redacts_credentials() {
    let server = TestServer::start(WebOptions::default()).await;
    let session = server.new_session().await;
    let (status, events) = server.chat(&session, &chat_body("你好")).await;
    assert_eq!(status, 200);
    assert_eq!(types(&events), vec!["reply", "done"]);
    assert_eq!(events[0]["text"], "你好，我是小满");
    for event in &events {
        assert!(
            !event.to_string().contains(API_KEY),
            "the API key never reaches the page"
        );
    }
}

#[tokio::test]
async fn the_reminder_tool_returns_an_importable_calendar() {
    let server = TestServer::start(WebOptions::default()).await;
    let session = server.new_session().await;
    let (status, events) = server.chat(&session, &chat_body("reminder")).await;
    assert_eq!(status, 200);
    assert_eq!(types(&events), vec!["reply", "reminder", "reply", "done"]);
    let reminder = &events[1];
    assert_eq!(reminder["text"], "喝水,带伞");
    assert!(reminder["at"].as_str().unwrap().starts_with("2099-11-0"));
    let calendar = reminder["ics"].as_str().unwrap();
    assert!(calendar.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
    assert!(calendar.ends_with("END:VCALENDAR\r\n"));
    assert!(calendar.contains("\r\nDTSTART:"));
    assert!(calendar.contains("\r\nSUMMARY:喝水\\,带伞\r\n"));
    assert!(calendar.contains("BEGIN:VALARM\r\nTRIGGER:PT0M"));
}

#[tokio::test]
async fn memory_writes_come_back_as_saved_notes() {
    let server = TestServer::start(WebOptions::default()).await;
    let session = server.new_session().await;
    let (status, events) = server.chat(&session, &chat_body("memory")).await;
    assert_eq!(status, 200);
    assert_eq!(types(&events), vec!["reply", "memory", "reply", "done"]);
    let saved = &events[1];
    assert_eq!(saved["action"], "saved");
    assert_eq!(saved["permalink"], "家");
    assert_eq!(saved["title"], "家");
    assert!(
        saved["markdown"]
            .as_str()
            .unwrap()
            .contains("- [地址] 北京")
    );
    assert!(saved["updatedAt"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn static_assets_are_served_with_the_page_headers() {
    let server = TestServer::start(WebOptions::default()).await;
    let index = server
        .client
        .get(format!("{}/", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(index.status(), 200);
    assert_eq!(index.headers()["content-type"], "text/html; charset=utf-8");
    assert!(
        index.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("connect-src 'self'")
    );
    assert_eq!(index.headers()["x-content-type-options"], "nosniff");
    assert_eq!(index.headers()["referrer-policy"], "no-referrer");
    let page = index.text().await.unwrap();
    assert!(page.contains("小满"));

    let script = server
        .client
        .get(format!("{}/app.js", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(script.status(), 200);
    assert_eq!(
        script.headers()["content-type"],
        "text/javascript; charset=utf-8"
    );
    assert!(script.text().await.unwrap().len() > 1_000);

    let claim = server
        .client
        .get(format!("{}/claim", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(claim.status(), 200);
    assert!(claim.text().await.unwrap().contains("领取"));

    let missing = server
        .client
        .get(format!("{}/missing.js", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
    assert_eq!(
        missing.json::<Value>().await.unwrap()["error"],
        "文件不存在"
    );

    // A URL parser would resolve `..` away before sending, so the server is asked by hand.
    for path in ["/%2e%2e/index.html", "/../index.html"] {
        let status = raw_get(server.port(), path).await;
        assert!(status.starts_with("HTTP/1.1 400"), "{path}: {status}");
    }
}

/// One hand-written request, so percent-encoded traversal reaches the server unchanged.
async fn raw_get(port: u16, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut buffer = Vec::new();
    socket.read_to_end(&mut buffer).await.unwrap();
    String::from_utf8_lossy(&buffer).to_string()
}

#[tokio::test]
async fn health_and_unknown_routes_follow_the_contract() {
    let server = TestServer::start(WebOptions::default()).await;
    let info: Value = server
        .client
        .get(format!("{}/api/info", server.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["name"], "Xiaoman");
    assert_eq!(info["version"], "1.1");
    assert_eq!(info["accessCodeRequired"], false);
    assert_eq!(info["memory"], "browser");
    assert_eq!(info["stdioMcp"], false);
    assert_eq!(info["classTokens"], false);

    let catalog: Value = server
        .client
        .get(format!("{}/api/catalog", server.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = catalog["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|server| server["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"amap"));
    assert!(ids.contains(&"12306"));
    let train = catalog["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|server| server["id"] == "12306")
        .unwrap();
    assert_eq!(train["available"], false);
    assert!(train["note"].as_str().unwrap().contains("Node.js"));

    let unknown = server
        .client
        .get(format!("{}/api/nope", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
    assert_eq!(
        unknown.json::<Value>().await.unwrap()["error"],
        "接口不存在"
    );

    let wrong_method = server
        .client
        .get(format!("{}/api/chat", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_method.status(), 405);
    assert_eq!(
        wrong_method.json::<Value>().await.unwrap()["error"],
        "请求方法不支持"
    );

    let expired = server
        .client
        .post(format!("{}/api/chat", server.base))
        .header("x-xiaoman-session", "missing")
        .json(&chat_body("你好"))
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), 404);
    assert_eq!(
        expired.json::<Value>().await.unwrap()["error"],
        "会话已过期"
    );
}

#[tokio::test]
async fn access_code_protects_every_api_except_info() {
    let options = WebOptions {
        access_code: Some("access".to_string()),
        ..Default::default()
    };
    let server = TestServer::start(options).await;
    let info: Value = server
        .client
        .get(format!("{}/api/info", server.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["accessCodeRequired"], true);
    let blocked = server
        .client
        .get(format!("{}/api/catalog", server.base))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 401);
    assert_eq!(
        blocked.json::<Value>().await.unwrap()["error"],
        "需要访问码"
    );
    let blocked = server
        .client
        .post(format!("{}/api/session", server.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 401);
    let allowed = server
        .client
        .get(format!("{}/api/catalog", server.base))
        .header("x-xiaoman-access", "access")
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);
}

#[tokio::test]
async fn malformed_and_oversized_bodies_get_safe_errors() {
    let server = TestServer::start(WebOptions::default()).await;
    let session = server.new_session().await;
    let malformed = server
        .client
        .post(format!("{}/api/chat", server.base))
        .header("x-xiaoman-session", &session)
        .body("{bad")
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);
    assert_eq!(
        malformed.json::<Value>().await.unwrap()["error"],
        "请求内容需要是合法 JSON 对象"
    );

    let oversized = server
        .client
        .post(format!("{}/api/chat", server.base))
        .header("x-xiaoman-session", &session)
        .body("x".repeat(xiaoman_web::MAX_BODY_BYTES + 1))
        .send()
        .await
        .unwrap();
    assert_eq!(oversized.status(), 413);
    assert_eq!(
        oversized.json::<Value>().await.unwrap()["error"],
        "请求内容超过 256KB"
    );
}

#[tokio::test]
async fn a_misconfigured_model_reports_an_error_event() {
    let server = TestServer::start(WebOptions::default()).await;
    let session = server.new_session().await;
    let (status, events) = server.chat(&session, &json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(types(&events), vec!["error", "done"]);
    assert_eq!(events[0]["message"], "请先配置模型地址、模型名称和 API Key");
}

#[tokio::test]
async fn sessions_are_capped() {
    use std::sync::atomic::{AtomicI64, Ordering};

    let options = WebOptions {
        max_sessions: 2,
        ..Default::default()
    };
    // A controlled clock keeps the least-recently-used eviction deterministic.
    let ticks = Arc::new(AtomicI64::new(0));
    let clock = {
        let ticks = ticks.clone();
        Arc::new(move || ticks.fetch_add(100, Ordering::SeqCst) + 100)
    };
    let server = WebServer::new(WebOptions {
        port: 0,
        allow_private: true,
        ..options
    })
    .with_llm(scripted())
    .with_clock(clock)
    .with_logger(Arc::new(|_line: &str| {}))
    .bind()
    .await
    .expect("bind");
    let base = format!("http://127.0.0.1:{}", server.port());
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client");
    let new_session = || {
        let client = client.clone();
        let base = base.clone();
        async move {
            let body: Value = client
                .post(format!("{base}/api/session"))
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            body["sessionId"].as_str().unwrap().to_string()
        }
    };
    let first = new_session().await;
    let _second = new_session().await;
    let _third = new_session().await;
    let expired = client
        .post(format!("{base}/api/chat"))
        .header("x-xiaoman-session", &first)
        .json(&chat_body("你好"))
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), 404);
    // The server is still healthy after evicting the least recently used session.
    let info = client.get(format!("{base}/api/info")).send().await.unwrap();
    assert_eq!(info.status(), 200);
}

#[tokio::test]
async fn a_second_turn_is_rejected_while_the_first_is_running() {
    let server = TestServer::start(WebOptions::default()).await;
    let session = server.new_session().await;
    let first = {
        let client = server.client.clone();
        let base = server.base.clone();
        let body = chat_body("slow");
        let session = session.clone();
        tokio::spawn(async move {
            client
                .post(format!("{base}/api/chat"))
                .header("x-xiaoman-session", &session)
                .json(&body)
                .send()
                .await
                .expect("first chat")
                .text()
                .await
                .expect("first stream")
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let busy = server
        .client
        .post(format!("{}/api/chat", server.base))
        .header("x-xiaoman-session", &session)
        .json(&chat_body("你好"))
        .send()
        .await
        .unwrap();
    assert_eq!(busy.status(), 409);
    assert_eq!(
        busy.json::<Value>().await.unwrap()["error"],
        "上一条消息还在处理"
    );

    let text = first.await.expect("the first turn finishes");
    let events = events(&text, reqwest::StatusCode::OK);
    assert_eq!(types(&events), vec!["reply", "done"]);
    assert_eq!(events[0]["text"], "慢回复");

    // The turn released the session, so the next message is accepted.
    let (status, events) = server.chat(&session, &chat_body("你好")).await;
    assert_eq!(status, 200);
    assert_eq!(types(&events), vec!["reply", "done"]);
}

/// Answers the MCP lifecycle without a socket, so the test does not depend on HTTP at all.
struct FakeTransport;

#[async_trait]
impl McpTransport for FakeTransport {
    async fn exchange(&self, message: Value) -> Result<Option<Value>, McpError> {
        if message.get("id").is_none_or(Value::is_null) {
            return Ok(None);
        }
        let result = match message["method"].as_str().unwrap_or_default() {
            "initialize" => json!({
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fake", "version": "1" },
            }),
            "tools/list" => json!({ "tools": [{
                "name": "read",
                "description": "读取",
                "inputSchema": { "type": "object" },
                "annotations": { "readOnlyHint": true },
            }] }),
            "tools/call" => {
                json!({ "content": [{ "type": "text", "text": "成功 mcp-secret-456" }] })
            }
            _ => json!({}),
        };
        Ok(Some(
            json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }),
        ))
    }

    fn close(&self) {}
}

#[tokio::test]
async fn mcp_tools_run_through_the_chat_stream_and_are_reported_by_the_test_route() {
    let server = WebServer::new(WebOptions {
        port: 0,
        allow_private: true,
        ..Default::default()
    })
    .with_llm(scripted())
    .with_transports(Arc::new(|_server: &McpServerConfig| {
        Ok(Arc::new(FakeTransport) as Arc<dyn McpTransport>)
    }))
    .with_logger(Arc::new(|_line: &str| {}))
    .bind()
    .await
    .expect("bind");
    let base = format!("http://127.0.0.1:{}", server.port());
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client");
    let body: Value = client
        .post(format!("{base}/api/session"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = body["sessionId"].as_str().unwrap().to_string();
    let request = json!({
        "message": "mcp",
        "model": { "baseUrl": "https://llm.invalid", "model": "fake", "apiKey": API_KEY },
        "mcp": {
            "keys": {},
            "custom": [{ "title": "测试服务", "url": "https://mcp.example/mcp", "confirm": "writes" }],
        },
    });

    let status: Value = client
        .post(format!("{base}/api/mcp/test"))
        .header("x-xiaoman-session", &session)
        .json(&request)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let reported = &status["servers"][0];
    assert_eq!(reported["name"], "custom-1");
    assert_eq!(reported["title"], "测试服务");
    assert_eq!(reported["connected"], true);
    assert_eq!(reported["toolCount"], 1);

    let response = client
        .post(format!("{base}/api/chat"))
        .header("x-xiaoman-session", &session)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    let events = events(&text, reqwest::StatusCode::OK);
    assert_eq!(types(&events), vec!["reply", "tool", "reply", "done"]);
    assert_eq!(events[1]["tool"], "custom-1__read");
    assert_eq!(events[1]["ok"], true);
    assert_eq!(events[1]["summary"], "测试服务·read 成功");
    assert!(
        !text.contains("mcp-secret-456"),
        "tool output never reaches the page"
    );
    assert!(
        !text.contains(API_KEY),
        "the API key never reaches the page"
    );
}
