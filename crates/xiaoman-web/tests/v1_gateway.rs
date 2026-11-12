//! Port of the Kotlin `ClassGatewayTest` for the `/v1` OpenAI-compatible passthrough: forwarding
//! with the instructor's key, metering, streaming, the limiter, body limits, timeouts, error
//! envelopes, redaction, and the usage shared with the page's class-token mode. The upstream is a
//! wiremock server that records what the gateway sends it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use xiaoman_api::{ChatMessage, ChatRequest, LlmClient, ModelConfig, ModelRole, Usage};
use xiaoman_llm::{ClientOptions, OpenAiCompatibleClient};
use xiaoman_web::class_gateway::ClassOptions;
use xiaoman_web::class_tokens::ClassTokenStore;
use xiaoman_web::memory::Clock;
use xiaoman_web::{BoundServer, WebOptions, WebServer};

/// The instructor's upstream key: the string that must never reach a client or a log.
const UPSTREAM_KEY: &str = "ark-instructor-secret-9f8e7d";

// ---- the fake upstream ----

#[derive(Clone, Debug)]
struct Seen {
    auth: Option<String>,
    body: Value,
}

struct FakeUpstream {
    key: String,
    seen: Mutex<Vec<Seen>>,
    /// The upstream reports usage unless a test turns it off.
    usage: AtomicBool,
    /// How long the upstream waits before answering; keeps requests in flight for the limiter test.
    delay_millis: AtomicUsize,
}

impl FakeUpstream {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            key: UPSTREAM_KEY.to_string(),
            seen: Mutex::new(Vec::new()),
            usage: AtomicBool::new(true),
            delay_millis: AtomicUsize::new(0),
        })
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn count(&self) -> usize {
        self.seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    fn answer(&self, auth: Option<String>, body: Value) -> ResponseTemplate {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Some(auth) = auth else {
            return json_reply(
                401,
                json!({ "error": { "code": "AuthenticationError", "message": "bad key" } }),
            );
        };
        if auth != format!("Bearer {}", self.key) {
            return json_reply(
                401,
                json!({ "error": { "code": "AuthenticationError", "message": "bad key" } }),
            );
        }
        match model.as_str() {
            "broken-model" => json_reply(
                400,
                json!({ "error": { "code": "InvalidParameter", "message": format!("image too large; header was {auth}") } }),
            ),
            "deny-model" => json_reply(
                401,
                json!({ "error": { "code": "AuthenticationError", "message": format!("key {} invalid", self.key) } }),
            ),
            _ if body.get("stream").and_then(Value::as_bool) == Some(true) => {
                self.stream(&model, &body)
            }
            _ => {
                let mut reply = json!({
                    "id": "c1",
                    "object": "chat.completion",
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "finish_reason": "stop",
                        "message": { "role": "assistant", "content": "你好，我是小满" },
                    }],
                });
                if self.usage.load(Ordering::SeqCst) {
                    reply["usage"] = json!({ "prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120 });
                }
                json_reply(200, reply)
            }
        }
    }

    /// Two chunks plus the usage-only chunk when the client asked for it, then `[DONE]`.
    fn stream(&self, model: &str, body: &Value) -> ResponseTemplate {
        let chunk = |value: Value| format!("data: {value}\n\n");
        let mut text = String::new();
        text.push_str(&chunk(json!({
            "id": "1", "object": "chat.completion.chunk", "model": model,
            "choices": [{ "index": 0, "delta": { "role": "assistant", "content": "你" } }],
        })));
        text.push_str(&chunk(json!({
            "id": "1", "object": "chat.completion.chunk", "model": model,
            "choices": [{ "index": 0, "delta": { "content": "好" }, "finish_reason": "stop" }],
        })));
        let wants = body
            .get("stream_options")
            .and_then(|options| options.get("include_usage"))
            .and_then(Value::as_bool)
            == Some(true);
        if wants && self.usage.load(Ordering::SeqCst) {
            text.push_str(&chunk(json!({
                "id": "1", "object": "chat.completion.chunk", "model": model, "choices": [],
                "usage": { "prompt_tokens": 30, "completion_tokens": 7, "total_tokens": 37 },
            })));
        }
        text.push_str("data: [DONE]\n\n");
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(text)
    }
}

impl Respond for FakeUpstream {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let auth = request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        self.seen
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(Seen {
                auth: auth.clone(),
                body: body.clone(),
            });
        let reply = self.answer(auth, body);
        let delay = self.delay_millis.load(Ordering::SeqCst);
        if delay == 0 {
            reply
        } else {
            reply.set_delay(Duration::from_millis(delay as u64))
        }
    }
}

struct UpstreamRoute(Arc<FakeUpstream>);

impl Respond for UpstreamRoute {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        self.0.respond(request)
    }
}

fn json_reply(status: u16, body: Value) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .insert_header("content-type", "application/json")
        .set_body_json(body)
}

async fn mount_upstream() -> (MockServer, Arc<FakeUpstream>) {
    let upstream = FakeUpstream::new();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(UpstreamRoute(upstream.clone()))
        .mount(&server)
        .await;
    (server, upstream)
}

// ---- the server under test ----

fn models() -> Vec<(String, String)> {
    vec![
        ("xiaoman-class".to_string(), "doubao-real-id".to_string()),
        ("broken".to_string(), "broken-model".to_string()),
        ("deny".to_string(), "deny-model".to_string()),
    ]
}

fn class_options(upstream: &str, file: &Path) -> ClassOptions {
    ClassOptions {
        reload_millis: 0,
        flush_millis: 60_000,
        ..ClassOptions::new(
            UPSTREAM_KEY,
            models(),
            upstream.to_string(),
            file.to_path_buf(),
        )
    }
}

struct Harness {
    base: String,
    client: reqwest::Client,
    logs: Arc<Mutex<Vec<String>>>,
    server: Option<BoundServer>,
}

impl Harness {
    async fn start(class: Option<ClassOptions>) -> Self {
        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let logger: xiaoman_web::server::Logger = {
            let logs = logs.clone();
            Arc::new(move |line: &str| {
                logs.lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(line.to_string())
            })
        };
        let server = WebServer::new(WebOptions {
            port: 0,
            allow_private: true,
            class,
            ..Default::default()
        })
        .with_logger(logger)
        .bind()
        .await
        .expect("the server binds");
        let base = format!("http://127.0.0.1:{}", server.port());
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("client");
        Self {
            base,
            client,
            logs,
            server: Some(server),
        }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Reply {
        let method: reqwest::Method = method.parse().expect("an HTTP method");
        let mut request = self.client.request(method, format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .expect("the request reaches the server");
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let text = response.text().await.unwrap_or_default();
        Reply {
            status,
            text,
            headers,
        }
    }

    fn logs(&self) -> Vec<String> {
        self.logs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// One turn of the page's class-token mode; returns the decoded SSE events.
    async fn chat_turn(&self, token: &str) -> Vec<Value> {
        let session = self
            .call("POST", "/api/session", None, Some(json!({})))
            .await;
        let id = session.json()["sessionId"]
            .as_str()
            .expect("a session id")
            .to_string();
        let response = self
            .client
            .post(format!("{}/api/chat", self.base))
            .header("x-xiaoman-session", id)
            .json(&json!({
                "message": "你好",
                "model": { "classToken": token },
                "mcp": { "keys": {}, "custom": [] },
            }))
            .send()
            .await
            .expect("the chat request reaches the server");
        let text = tokio::time::timeout(Duration::from_secs(20), response.text())
            .await
            .expect("the turn ends")
            .unwrap_or_default();
        text.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|data| serde_json::from_str::<Value>(data).ok())
            .collect()
    }

    async fn shutdown(mut self) {
        if let Some(server) = self.server.take() {
            server.shutdown().await;
        }
    }
}

struct Reply {
    status: u16,
    text: String,
    headers: reqwest::header::HeaderMap,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    fn code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn kind(&self) -> String {
        self.json()["error"]["type"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

// ---- tokens and usage ----

fn clock() -> Clock {
    Arc::new(xiaoman_api::now_millis)
}

fn admin_stdout(file: &Path, args: &[&str]) -> String {
    let mut argv: Vec<String> = args.iter().map(|argument| argument.to_string()).collect();
    argv.push("--file".to_string());
    argv.push(file.display().to_string());
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = xiaoman_web::tokens_admin::run(&argv, &mut out, &mut err, &|_| None, clock());
    assert_eq!(
        code,
        0,
        "admin {args:?} failed: {}",
        String::from_utf8_lossy(&err)
    );
    String::from_utf8(out).expect("utf-8 stdout")
}

fn admin(file: &Path, args: &[&str]) {
    admin_stdout(file, args);
}

/// A fresh token file with `count` tokens; returns the temp dir that owns it, the path and the
/// plaintexts in file order.
fn tokens_with(count: usize, extra: &[&str]) -> (tempfile::TempDir, PathBuf, Vec<String>) {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("class-tokens.json");
    let owned: Vec<String> = std::iter::once("generate".to_string())
        .chain(std::iter::once(count.to_string()))
        .chain(extra.iter().map(|argument| argument.to_string()))
        .collect();
    let args: Vec<&str> = owned.iter().map(String::as_str).collect();
    let printed = admin_stdout(&file, &args);
    let tokens = printed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            line.rsplit_once('\t')
                .expect("label and plaintext")
                .1
                .to_string()
        })
        .collect();
    (dir, file, tokens)
}

fn without_tokens() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("class-tokens.json");
    (dir, file)
}

/// `(used_total, requests)` as the token file (and the pending writes) see them.
fn measured(file: &Path, token: &str) -> (i64, i64) {
    let store = ClassTokenStore::new(file.to_path_buf(), 480, clock(), 0);
    let found = store.find(token).expect("the token is in the file");
    (found.used_total, found.requests)
}

// ---- request bodies ----

fn chat_body(model: &str, stream: bool, image: bool, include_usage: bool) -> Value {
    let content = if image {
        json!([
            { "type": "text", "text": "看截图" },
            { "type": "image_url", "image_url": {
                "url": format!("data:image/png;base64,{}", "A".repeat(300_000)),
            } },
        ])
    } else {
        json!("你好")
    };
    let mut body = json!({
        "model": model,
        "response_format": { "type": "json_object" },
        "messages": [{ "role": "user", "content": content }],
    });
    if stream {
        body["stream"] = json!(true);
    }
    if include_usage {
        body["stream_options"] = json!({ "include_usage": true });
    }
    body
}

fn assert_no_leak(logs: &[String], tokens: &[String]) {
    for line in logs {
        assert!(
            !line.contains(UPSTREAM_KEY),
            "a log leaked the upstream key: {line}"
        );
        for token in tokens {
            assert!(!line.contains(token), "a log leaked a class token: {line}");
        }
    }
}

// ---- the tests ----

#[tokio::test]
async fn non_streaming_forwards_with_the_instructor_key_and_meters_usage() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(3, &[]);
    let harness = Harness::start(Some(class_options(
        &format!("{}/api/v3", upstream.uri()),
        &file,
    )))
    .await;

    let reply = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text);
    assert_eq!(
        reply.header("x-content-type-options").as_deref(),
        Some("nosniff"),
        "the page headers still apply"
    );
    assert_eq!(
        reply.json()["choices"][0]["message"]["content"],
        "你好，我是小满"
    );

    let seen = up.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].auth.as_deref(),
        Some(format!("Bearer {UPSTREAM_KEY}").as_str())
    );
    assert_eq!(
        seen[0].body["model"], "doubao-real-id",
        "the alias is replaced by the real model id"
    );
    assert_eq!(
        seen[0].body["response_format"]["type"], "json_object",
        "other fields pass through"
    );
    assert!(seen[0].body.get("stream_options").is_none());

    let info = harness.call("GET", "/api/info", None, None).await;
    assert_eq!(info.json()["classTokens"], true);

    let logs = harness.logs();
    harness.shutdown().await;
    let (used, requests) = measured(&file, &tokens[0]);
    assert_eq!(
        (used, requests),
        (120, 1),
        "the usage is written back when the server stops"
    );
    let line = logs
        .iter()
        .find(|line| line.starts_with("POST /v1/chat/completions"))
        .expect("the request is logged");
    let prefix: String = tokens[0].chars().take(6).collect();
    assert!(
        line.contains(&format!("学员 01({prefix})")) && line.contains("tokens=120"),
        "{line}"
    );
    assert_no_leak(&logs, &tokens);
}

#[tokio::test]
async fn streaming_requests_usage_but_only_forwards_it_when_asked() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(3, &[]);
    let harness = Harness::start(Some(class_options(
        &format!("{}/api/v3", upstream.uri()),
        &file,
    )))
    .await;

    let plain = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("xiaoman-class", true, false, false)),
        )
        .await;
    assert_eq!(plain.status, 200, "{}", plain.text);
    assert!(
        plain
            .header("content-type")
            .expect("a content type")
            .starts_with("text/event-stream")
    );
    let data: Vec<&str> = plain
        .text
        .lines()
        .filter(|line| line.starts_with("data: "))
        .collect();
    assert_eq!(data.len(), 3, "{}", plain.text);
    assert_eq!(data[2], "data: [DONE]");
    assert!(
        !plain.text.contains("usage"),
        "the client did not ask for the usage chunk"
    );
    let first = up.seen();
    assert_eq!(
        first.last().expect("a request").body["stream_options"]["include_usage"],
        true
    );

    let asked = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[1]),
            Some(chat_body("xiaoman-class", true, false, true)),
        )
        .await;
    assert!(asked.text.contains("\"total_tokens\":37"), "{}", asked.text);

    up.usage.store(false, Ordering::SeqCst);
    let estimated = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[2]),
            Some(chat_body("xiaoman-class", true, false, false)),
        )
        .await;
    assert_eq!(estimated.status, 200, "{}", estimated.text);

    let logs = harness.logs();
    harness.shutdown().await;
    assert_eq!(measured(&file, &tokens[0]).0, 37);
    assert_eq!(measured(&file, &tokens[1]).0, 37);
    let estimate = measured(&file, &tokens[2]).0;
    assert!(
        (1..=20).contains(&estimate),
        "estimated from the text: {estimate}"
    );
    assert!(
        logs.iter().any(|line| line.contains("stream tokens=37")),
        "the stream reports the upstream usage: {logs:?}"
    );
    assert!(logs.iter().any(|line| line.contains("(估算)")), "{logs:?}");
    let requests = logs
        .iter()
        .filter(|line| line.starts_with("POST /v1/chat/completions"))
        .count();
    assert_eq!(
        requests, 3,
        "one log line per request, all from the gateway: {logs:?}"
    );
    assert_no_leak(&logs, &tokens);
}

#[tokio::test]
async fn a_reply_without_usage_is_estimated() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(1, &[]);
    up.usage.store(false, Ordering::SeqCst);
    let harness = Harness::start(Some(class_options(
        &format!("{}/api/v3", upstream.uri()),
        &file,
    )))
    .await;

    let reply = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text);
    let logs = harness.logs();
    harness.shutdown().await;
    // "你好" is 6 bytes (2 + 4 fixed) and the reply is 7 characters.
    assert_eq!(measured(&file, &tokens[0]).0, 13);
    assert!(
        logs.iter().any(|line| line.contains("tokens=13(估算)")),
        "{logs:?}"
    );
}

#[tokio::test]
async fn images_pass_through_and_the_body_size_is_capped() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(1, &[]);
    let class = ClassOptions {
        max_body_bytes: 1024 * 1024,
        ..class_options(&format!("{}/api/v3", upstream.uri()), &file)
    };
    let harness = Harness::start(Some(class)).await;

    let reply = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("xiaoman-class", false, true, false)),
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text);
    let seen = up.seen();
    let parts = seen[0].body["messages"][0]["content"]
        .as_array()
        .expect("content parts");
    assert_eq!(parts[1]["type"], "image_url");
    assert!(
        parts[1]["image_url"]["url"]
            .as_str()
            .expect("a url")
            .starts_with("data:image/png;base64,")
    );

    let huge = json!({
        "model": "xiaoman-class",
        "messages": [{ "role": "user", "content": "a".repeat(1024 * 1024 + 10) }],
    });
    let big = harness
        .call("POST", "/v1/chat/completions", Some(&tokens[0]), Some(huge))
        .await;
    assert_eq!(big.status, 413, "{}", big.text);
    assert_eq!(big.code(), "request_too_large");
    assert_eq!(
        up.count(),
        1,
        "the oversized request never reached the upstream"
    );
}

#[tokio::test]
async fn chunked_bodies_over_the_limit_are_drained_and_refused() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(1, &[]);
    let class = ClassOptions {
        max_body_bytes: 64 * 1024,
        ..class_options(&format!("{}/api/v3", upstream.uri()), &file)
    };
    let harness = Harness::start(Some(class)).await;
    let port = harness
        .base
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .expect("a port");

    // No Content-Length, so the size is only known while reading: the body is read and dropped.
    let response = raw_chunked_post(port, "/v1/chat/completions", &tokens[0], 200 * 1024).await;
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    assert!(response.contains("request_too_large"), "{response}");
    assert_eq!(up.count(), 0);
    harness.shutdown().await;
}

/// One hand-written chunked request, so the body has no `Content-Length` header.
async fn raw_chunked_post(port: u16, path: &str, token: &str, payload: usize) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    socket.write_all(head.as_bytes()).await.expect("head");
    let parts = [
        br#"{"model":"xiaoman-class","messages":[{"role":"user","content":""#.to_vec(),
        "a".repeat(payload).into_bytes(),
        br#""}]}"#.to_vec(),
    ];
    for part in parts {
        socket
            .write_all(format!("{:x}\r\n", part.len()).as_bytes())
            .await
            .expect("a chunk size");
        socket.write_all(&part).await.expect("a chunk");
        socket.write_all(b"\r\n").await.expect("a chunk end");
    }
    socket
        .write_all(b"0\r\n\r\n")
        .await
        .expect("the last chunk");
    let mut buffer = Vec::new();
    socket.read_to_end(&mut buffer).await.expect("read");
    String::from_utf8_lossy(&buffer).to_string()
}

#[tokio::test]
async fn invalid_revoked_expired_and_over_quota_tokens_get_openai_errors() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(4, &["--daily", "100"]);
    let harness = Harness::start(Some(class_options(
        &format!("{}/api/v3", upstream.uri()),
        &file,
    )))
    .await;

    let missing = harness
        .call(
            "POST",
            "/v1/chat/completions",
            None,
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(
        (missing.status, missing.code().as_str()),
        (401, "missing_token")
    );

    let bad = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&format!("xm-{}", "z".repeat(20))),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!((bad.status, bad.code().as_str()), (401, "invalid_token"));
    assert_eq!(bad.kind(), "authentication_error");

    admin(&file, &["revoke", "学员 01"]);
    let revoked = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(
        revoked.code(),
        "token_revoked",
        "revocation applies while running"
    );
    let models = harness
        .call("GET", "/v1/models", Some(&tokens[0]), None)
        .await;
    assert_eq!(models.code(), "token_revoked");

    admin(
        &file,
        &["limit", "学员 02", "--expires", "2000-01-01 00:00"],
    );
    let expired = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[1]),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(expired.code(), "token_expired");

    let admitted = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[2]),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(
        admitted.status, 200,
        "120 tokens are still under the daily 100"
    );
    let over = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[2]),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(
        (over.status, over.code().as_str()),
        (429, "daily_quota_exceeded")
    );
    assert_eq!(over.kind(), "insufficient_quota");

    admin(&file, &["revoke-all"]);
    let after = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[3]),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(after.code(), "token_revoked");
    assert_eq!(
        up.count(),
        1,
        "only the admitted request reached the upstream"
    );

    let logs = harness.logs();
    harness.shutdown().await;
    assert_no_leak(&logs, &tokens);
}

#[tokio::test]
async fn the_whitelist_hides_real_ids_and_the_models_list_is_the_alias_list() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(1, &[]);
    let harness = Harness::start(Some(class_options(
        &format!("{}/api/v3", upstream.uri()),
        &file,
    )))
    .await;

    let models = harness
        .call("GET", "/v1/models", Some(&tokens[0]), None)
        .await;
    assert_eq!(models.status, 200, "{}", models.text);
    let listed = models.json();
    assert_eq!(listed["object"], "list");
    let ids: Vec<&str> = listed["data"]
        .as_array()
        .expect("data")
        .iter()
        .map(|model| model["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(ids, vec!["xiaoman-class", "broken", "deny"]);
    assert_eq!(listed["data"][0]["object"], "model");
    assert_eq!(listed["data"][0]["created"], 0);
    assert_eq!(listed["data"][0]["owned_by"], "xiaoman-class");
    assert!(
        !models.text.contains("doubao-real-id"),
        "real ids stay on the server"
    );

    let denied = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("doubao-real-id", false, false, false)),
        )
        .await;
    assert_eq!(
        (denied.status, denied.code().as_str()),
        (400, "model_not_allowed")
    );
    assert_eq!(up.count(), 0);

    let anonymous = harness.call("GET", "/v1/models", None, None).await;
    assert_eq!(anonymous.status, 401);
    let wrong_method = harness
        .call("DELETE", "/v1/models", Some(&tokens[0]), None)
        .await;
    assert_eq!(
        (wrong_method.status, wrong_method.code().as_str()),
        (405, "method_not_allowed")
    );

    // Anything else under `/v1` is not an endpoint; the bare `/v1/` is one of them.
    for path in ["/v1/embeddings", "/v1/"] {
        let missing = harness
            .call(
                "POST",
                path,
                Some(&tokens[0]),
                Some(chat_body("xiaoman-class", false, false, false)),
            )
            .await;
        assert_eq!(
            (missing.status, missing.code().as_str()),
            (404, "not_found"),
            "{path}"
        );
    }
    assert_eq!(up.count(), 0);
}

#[tokio::test]
async fn upstream_errors_never_reveal_the_instructor_key() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(1, &[]);
    let harness = Harness::start(Some(class_options(
        &format!("{}/api/v3", upstream.uri()),
        &file,
    )))
    .await;

    let broken = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("broken", false, false, false)),
        )
        .await;
    assert_eq!(
        (broken.status, broken.code().as_str()),
        (400, "InvalidParameter")
    );
    assert!(broken.text.contains("image too large"), "{}", broken.text);
    assert!(!broken.text.contains(UPSTREAM_KEY), "{}", broken.text);

    let auth = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("deny", false, false, false)),
        )
        .await;
    assert_eq!(
        (auth.status, auth.code().as_str()),
        (502, "upstream_auth_failed")
    );
    assert!(!auth.text.contains(UPSTREAM_KEY), "{}", auth.text);

    let stream = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("broken", true, false, false)),
        )
        .await;
    assert_eq!(stream.status, 400, "{}", stream.text);
    assert!(!stream.text.contains(UPSTREAM_KEY));

    let logs = harness.logs();
    harness.shutdown().await;
    assert_eq!(up.count(), 3);
    assert_no_leak(&logs, &tokens);
}

#[tokio::test]
async fn a_stream_reaches_the_client_chunk_by_chunk() {
    let (_dir, file, tokens) = tokens_with(1, &[]);
    let (port, upstream) = trickling_upstream().await;
    let harness = Harness::start(Some(class_options(
        &format!("http://127.0.0.1:{port}/api/v3"),
        &file,
    )))
    .await;

    let started = std::time::Instant::now();
    let mut response = harness
        .client
        .post(format!("{}/v1/chat/completions", harness.base))
        .header("authorization", format!("Bearer {}", tokens[0]))
        .json(&chat_body("xiaoman-class", true, false, false))
        .send()
        .await
        .expect("the stream starts");
    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let first = response
        .chunk()
        .await
        .expect("a chunk")
        .expect("the first chunk");
    let first = String::from_utf8_lossy(&first).to_string();
    assert!(first.contains('你'), "{first}");
    assert!(
        started.elapsed() < Duration::from_millis(700),
        "the first chunk is forwarded while the upstream is still writing: {:?}",
        started.elapsed()
    );

    let mut rest = Vec::new();
    while let Some(chunk) = response.chunk().await.expect("a chunk") {
        rest.extend_from_slice(&chunk);
    }
    let rest = String::from_utf8_lossy(&rest).to_string();
    assert!(rest.contains('好') && rest.contains("[DONE]"), "{rest}");
    harness.shutdown().await;
    upstream.await.expect("the upstream task ends");
}

/// An upstream that sends the first SSE chunk, waits a second, then the rest: a buffered gateway
/// would deliver everything at once and miss the deadline above.
async fn trickling_upstream() -> (u16, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("an address").port();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("a connection");
        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = socket.read(&mut chunk).await.expect("a request");
            if read == 0 {
                return;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n")
            .await
            .expect("headers");
        write_chunk(
            &mut socket,
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"你\"}}]}\n\n",
        )
        .await;
        tokio::time::sleep(Duration::from_millis(1_000)).await;
        write_chunk(
            &mut socket,
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"好\"},\"finish_reason\":\"stop\"}]}\n\n",
        )
        .await;
        write_chunk(&mut socket, "data: [DONE]\n\n").await;
        socket.write_all(b"0\r\n\r\n").await.expect("the end");
        let _ = socket.shutdown().await;
    });
    (port, task)
}

async fn write_chunk(socket: &mut tokio::net::TcpStream, data: &str) {
    use tokio::io::AsyncWriteExt;
    socket
        .write_all(format!("{:x}\r\n", data.len()).as_bytes())
        .await
        .expect("a chunk size");
    socket.write_all(data.as_bytes()).await.expect("a chunk");
    socket.write_all(b"\r\n").await.expect("a chunk end");
    socket.flush().await.expect("flush");
}

#[tokio::test]
async fn a_slow_upstream_times_out_and_an_unreachable_one_is_a_502() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(2, &[]);

    up.delay_millis.store(3_000, Ordering::SeqCst);
    let slow = ClassOptions {
        timeout_seconds: 1,
        ..class_options(&format!("{}/api/v3", upstream.uri()), &file)
    };
    let harness = Harness::start(Some(slow)).await;
    let timed_out = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(
        (timed_out.status, timed_out.code().as_str()),
        (504, "upstream_timeout")
    );
    harness.shutdown().await;

    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("an address").port()
    };
    let harness = Harness::start(Some(class_options(
        &format!("http://127.0.0.1:{closed}/api/v3"),
        &file,
    )))
    .await;
    let unreachable = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[1]),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(
        (unreachable.status, unreachable.code().as_str()),
        (502, "upstream_unavailable")
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn per_token_rate_concurrency_and_the_global_cap() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(3, &[]);
    let url = format!("{}/api/v3", upstream.uri());

    {
        let class = ClassOptions {
            per_token_per_minute: 2,
            ..class_options(&url, &file)
        };
        let harness = Harness::start(Some(class)).await;
        for expected in [200u16, 200] {
            let reply = harness
                .call(
                    "POST",
                    "/v1/chat/completions",
                    Some(&tokens[0]),
                    Some(chat_body("xiaoman-class", false, false, false)),
                )
                .await;
            assert_eq!(reply.status, expected, "{}", reply.text);
        }
        let limited = harness
            .call(
                "POST",
                "/v1/chat/completions",
                Some(&tokens[0]),
                Some(chat_body("xiaoman-class", false, false, false)),
            )
            .await;
        assert_eq!(
            (limited.status, limited.code().as_str()),
            (429, "rate_limit_exceeded")
        );
        let wait: i64 = limited
            .header("retry-after")
            .expect("a Retry-After")
            .parse()
            .expect("seconds");
        assert!((1..=60).contains(&wait), "{wait}");
        let other = harness
            .call(
                "POST",
                "/v1/chat/completions",
                Some(&tokens[1]),
                Some(chat_body("xiaoman-class", false, false, false)),
            )
            .await;
        assert_eq!(other.status, 200, "other tokens are not affected");
        harness.shutdown().await;
    }

    {
        let class = ClassOptions {
            per_token_concurrency: 1,
            max_concurrent: 2,
            ..class_options(&url, &file)
        };
        let harness = Harness::start(Some(class)).await;
        up.delay_millis.store(600, Ordering::SeqCst);
        let base = up.count();
        let first = tokio::spawn({
            let client = harness.client.clone();
            let base_url = harness.base.clone();
            let token = tokens[0].clone();
            async move {
                client
                    .post(format!("{base_url}/v1/chat/completions"))
                    .header("authorization", format!("Bearer {token}"))
                    .json(&chat_body("xiaoman-class", false, false, false))
                    .send()
                    .await
                    .expect("the first request")
                    .status()
                    .as_u16()
            }
        });
        let second = tokio::spawn({
            let client = harness.client.clone();
            let base_url = harness.base.clone();
            let token = tokens[1].clone();
            async move {
                client
                    .post(format!("{base_url}/v1/chat/completions"))
                    .header("authorization", format!("Bearer {token}"))
                    .json(&chat_body("xiaoman-class", false, false, false))
                    .send()
                    .await
                    .expect("the second request")
                    .status()
                    .as_u16()
            }
        });
        wait_for(|| up.count() >= base + 2).await;
        let busy = harness
            .call(
                "POST",
                "/v1/chat/completions",
                Some(&tokens[0]),
                Some(chat_body("xiaoman-class", false, false, false)),
            )
            .await;
        assert_eq!(busy.code(), "too_many_concurrent_requests");
        let global = harness
            .call(
                "POST",
                "/v1/chat/completions",
                Some(&tokens[2]),
                Some(chat_body("xiaoman-class", false, false, false)),
            )
            .await;
        assert_eq!(global.code(), "server_busy");

        assert_eq!(first.await.expect("the first reply"), 200);
        assert_eq!(second.await.expect("the second reply"), 200);
        up.delay_millis.store(0, Ordering::SeqCst);
        let after = harness
            .call(
                "POST",
                "/v1/chat/completions",
                Some(&tokens[2]),
                Some(chat_body("xiaoman-class", false, false, false)),
            )
            .await;
        assert_eq!(after.status, 200, "the slots are released");
        harness.shutdown().await;
    }
}

async fn wait_for(condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for the upstream"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn the_feature_is_off_without_config_or_a_token_file() {
    let (upstream, _up) = mount_upstream().await;
    let (_dir, file) = without_tokens();

    let off = Harness::start(None).await;
    let info = off.call("GET", "/api/info", None, None).await;
    assert_eq!(info.json()["classTokens"], false);
    assert_eq!(
        off.call("GET", "/v1/models", Some("xm-x"), None)
            .await
            .status,
        404
    );
    off.shutdown().await;

    let harness = Harness::start(Some(class_options(
        &format!("{}/api/v3", upstream.uri()),
        &file,
    )))
    .await;
    let info = harness.call("GET", "/api/info", None, None).await;
    assert_eq!(
        info.json()["classTokens"],
        false,
        "configured but no token file yet"
    );
    let fall_through = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some("xm-x"),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(
        fall_through.status, 405,
        "falls through to the static files as before"
    );

    let printed = admin_stdout(&file, &["generate", "1"]);
    let token = printed
        .trim()
        .rsplit_once('\t')
        .expect("label and plaintext")
        .1
        .to_string();
    let info = harness.call("GET", "/api/info", None, None).await;
    assert_eq!(
        info.json()["classTokens"],
        true,
        "a token file created while running turns it on"
    );
    let reply = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&token),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text);
}

#[tokio::test]
async fn deep_thinking_is_off_by_default_unless_the_client_or_config_says_otherwise() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(1, &[]);
    let url = format!("{}/api/v3", upstream.uri());
    let harness = Harness::start(Some(class_options(&url, &file))).await;

    let reply = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text);
    assert_eq!(
        up.seen().last().expect("a request").body["thinking"]["type"],
        "disabled"
    );
    let streaming = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("xiaoman-class", true, false, false)),
        )
        .await;
    assert_eq!(streaming.status, 200, "{}", streaming.text);
    assert_eq!(
        up.seen().last().expect("a request").body["thinking"]["type"],
        "disabled",
        "streaming too"
    );

    let mut asked = chat_body("xiaoman-class", false, false, false);
    asked["thinking"] = json!({ "type": "enabled" });
    assert_eq!(
        harness
            .call(
                "POST",
                "/v1/chat/completions",
                Some(&tokens[0]),
                Some(asked)
            )
            .await
            .status,
        200
    );
    assert_eq!(
        up.seen().last().expect("a request").body["thinking"]["type"],
        "enabled",
        "an explicit client choice wins"
    );
    harness.shutdown().await;

    let keep = ClassOptions {
        thinking: None,
        ..class_options(&url, &file)
    };
    let harness = Harness::start(Some(keep)).await;
    assert_eq!(
        harness
            .call(
                "POST",
                "/v1/chat/completions",
                Some(&tokens[0]),
                Some(chat_body("xiaoman-class", false, false, false))
            )
            .await
            .status,
        200
    );
    assert!(
        up.seen()
            .last()
            .expect("a request")
            .body
            .get("thinking")
            .is_none(),
        "keep mode forwards the request unchanged"
    );
    harness.shutdown().await;

    let vars = |values: Vec<(&'static str, &'static str)>| {
        move |name: &str| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    };
    let base = vec![
        ("XIAOMAN_CLASS_UPSTREAM_KEY", "k"),
        ("XIAOMAN_CLASS_MODELS", "a=b"),
    ];
    let mut with = base.clone();
    let (options, _) = ClassOptions::from_env(&vars(with.clone()), None).expect("valid");
    assert_eq!(
        options.expect("configured").thinking.as_deref(),
        Some("disabled")
    );
    with.push(("XIAOMAN_CLASS_THINKING", "keep"));
    let (options, _) = ClassOptions::from_env(&vars(with), None).expect("valid");
    assert_eq!(options.expect("configured").thinking, None);
    let mut with = base.clone();
    with.push(("XIAOMAN_CLASS_THINKING", "auto"));
    let (options, _) = ClassOptions::from_env(&vars(with), None).expect("valid");
    assert_eq!(
        options.expect("configured").thinking.as_deref(),
        Some("auto")
    );
    let mut with = base;
    with.push(("XIAOMAN_CLASS_THINKING", "off"));
    assert_eq!(
        ClassOptions::from_env(&vars(with), None).unwrap_err(),
        "XIAOMAN_CLASS_THINKING 只能是 disabled/enabled/auto/keep"
    );
}

#[tokio::test]
async fn a_tokens_usage_is_shared_between_the_page_and_v1() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(2, &["--daily", "200"]);
    let harness = Harness::start(Some(class_options(
        &format!("{}/api/v3", upstream.uri()),
        &file,
    )))
    .await;

    let first = harness.chat_turn(&tokens[0]).await;
    let kinds: Vec<&str> = first
        .iter()
        .map(|event| event["type"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(kinds, vec!["reply", "done"], "{first:?}");
    assert_eq!(first[0]["text"], "你好，我是小满");
    let seen = up.seen();
    assert_eq!(
        seen[0].auth.as_deref(),
        Some(format!("Bearer {UPSTREAM_KEY}").as_str())
    );
    assert_eq!(seen[0].body["model"], "doubao-real-id");

    // The page's 120 and this 120 exceed the daily 200, so the next page turn is refused.
    let reply = harness
        .call(
            "POST",
            "/v1/chat/completions",
            Some(&tokens[0]),
            Some(chat_body("xiaoman-class", false, false, false)),
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text);
    let over = harness.chat_turn(&tokens[0]).await;
    assert_eq!(over[0]["type"], "error", "{over:?}");
    assert!(
        over[0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("额度"),
        "{over:?}"
    );

    admin(&file, &["revoke", "学员 02"]);
    let revoked = harness.chat_turn(&tokens[1]).await;
    assert!(
        revoked[0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("作废"),
        "{revoked:?}"
    );
    let unknown = harness.chat_turn(&format!("xm-{}", "q".repeat(20))).await;
    assert!(
        unknown[0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("无效"),
        "{unknown:?}"
    );
    assert_eq!(up.count(), 2);

    let logs = harness.logs();
    harness.shutdown().await;
    assert_eq!(measured(&file, &tokens[0]).0, 240);
    assert!(
        logs.iter()
            .any(|line| line.contains("WEB 课堂模式") && line.contains("学员 01")),
        "{logs:?}"
    );
    assert_no_leak(&logs, &tokens);
}

#[tokio::test]
async fn an_openai_client_uses_the_token_as_its_key_against_v1() {
    let (upstream, up) = mount_upstream().await;
    let (_dir, file, tokens) = tokens_with(1, &[]);
    let harness = Harness::start(Some(class_options(
        &format!("{}/api/v3", upstream.uri()),
        &file,
    )))
    .await;

    // Exactly what the app builds from its settings: Base URL …/v1/, key = token, model = a whitelist name.
    let client = OpenAiCompatibleClient::with_options(
        ModelConfig::new(
            ModelRole::Operator,
            format!("{}/v1/", harness.base),
            "xiaoman-class",
            tokens[0].clone(),
        ),
        ClientOptions {
            max_retries: 0,
            ..Default::default()
        },
    );
    let reply = client
        .complete(ChatRequest {
            json_mode: true,
            ..ChatRequest::new(vec![ChatMessage::user("点哪里")])
        })
        .await
        .expect("the reply");
    assert_eq!(reply.text, "你好，我是小满");
    assert_eq!(
        reply.usage,
        Some(Usage {
            prompt_tokens: 100,
            completion_tokens: 20
        })
    );
    let sent = up.seen();
    assert_eq!(sent[0].body["model"], "doubao-real-id");
    assert_eq!(sent[0].body["response_format"]["type"], "json_object");

    admin(&file, &["revoke", "学员 01"]);
    let error = client
        .complete(ChatRequest::new(vec![ChatMessage::user("hi")]))
        .await
        .expect_err("a revoked token is refused");
    assert_eq!(error.status_code, Some(401));
    assert_eq!(error.provider_code.as_deref(), Some("token_revoked"));
}
