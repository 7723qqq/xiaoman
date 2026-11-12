//! Port of the Kotlin `ForumClaimsTest`: self-service claiming against a Discourse-shaped forum
//! (wiremock), the one-per-account and total limits, the filters, and the metered chat turn a claimed
//! token enables.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use percent_encoding::percent_decode_str;
use serde_json::{Value, json};
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use xiaoman_web::claims::{ClaimConfig, active_claims};
use xiaoman_web::class_gateway::ClassOptions;
use xiaoman_web::class_tokens::{ClassToken, ClassTokenCodec, ClassTokenStore, ForumClaim};
use xiaoman_web::memory::Clock;
use xiaoman_web::{BoundServer, WebOptions, WebServer};

/// The instructor's topic in the fake forum.
const TOPIC: i64 = 42;
/// `Instant.parse("2026-11-06T06:00:00Z")`, the moment the Kotlin test starts its clock at.
const START: &str = "2026-11-06T06:00:00Z";
const UPSTREAM_KEY: &str = "ark-instructor-secret-claims";

fn start_millis() -> i64 {
    chrono::DateTime::parse_from_rfc3339(START)
        .expect("a fixed instant")
        .timestamp_millis()
}

fn instant_text(millis: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis)
        .expect("a valid instant")
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[derive(Debug, Clone)]
struct FakeUser {
    id: i64,
    username: String,
    created_at: i64,
    trust: i32,
}

struct ForumState {
    users: HashMap<String, FakeUser>,
    posts: Vec<Value>,
    next_post: i64,
}

/// A Discourse-shaped forum: `/u/{name}.json`, `/t/{id}.json` and `/t/{id}/posts.json?post_ids[]=…`.
struct FakeForum {
    state: Mutex<ForumState>,
    requests: AtomicUsize,
    post_requests: AtomicUsize,
    failing: AtomicBool,
}

impl FakeForum {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ForumState {
                users: HashMap::new(),
                posts: Vec::new(),
                next_post: 1000,
            }),
            requests: AtomicUsize::new(0),
            post_requests: AtomicUsize::new(0),
            failing: AtomicBool::new(false),
        })
    }

    fn state(&self) -> MutexGuard<'_, ForumState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn user(&self, id: i64, username: &str, created_at: i64, trust: i32) -> FakeUser {
        let user = FakeUser {
            id,
            username: username.to_string(),
            created_at,
            trust,
        };
        self.state()
            .users
            .insert(username.to_lowercase(), user.clone());
        user
    }

    /// A reply whose rendered HTML is the escaped raw text.
    fn reply(&self, user: &FakeUser, raw: &str, at: i64) {
        let cooked = format!(
            "<p>{}</p>",
            raw.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        );
        self.reply_full(user, raw, &cooked, at, 1, false);
    }

    fn reply_full(
        &self,
        user: &FakeUser,
        raw: &str,
        cooked: &str,
        at: i64,
        post_type: i64,
        hidden: bool,
    ) {
        let mut state = self.state();
        let id = state.next_post;
        state.next_post += 1;
        let number = state.posts.len() + 1;
        state.posts.push(json!({
            "id": id,
            "post_number": number,
            "user_id": user.id,
            "username": user.username,
            "raw": raw,
            "cooked": cooked,
            "post_type": post_type,
            "hidden": hidden,
            "deleted_at": null,
            "created_at": instant_text(at),
            "updated_at": instant_text(at),
            "trust_level": user.trust,
        }));
    }

    /// Filler replies by an unrelated account, older than any phrase.
    fn filler(&self, count: usize, at: i64) {
        let existing = self.state().users.get("filler").cloned();
        let user = match existing {
            Some(user) => user,
            None => self.user(9999, "filler", start_millis(), 1),
        };
        for index in 0..count {
            self.reply(&user, &format!("路过 {index}"), at);
        }
    }

    fn respond(&self, request: &Request) -> ResponseTemplate {
        self.requests.fetch_add(1, Ordering::SeqCst);
        if self.failing.load(Ordering::SeqCst) {
            return ResponseTemplate::new(500).set_body_string("{\"errors\":[\"boom\"]}");
        }
        let raw_path = percent_decode_str(request.url.path())
            .decode_utf8_lossy()
            .to_string();
        if let Some(name) = raw_path
            .strip_prefix("/u/")
            .and_then(|rest| rest.strip_suffix(".json"))
        {
            let state = self.state();
            let Some(user) = state.users.get(&name.to_lowercase()).cloned() else {
                return not_found();
            };
            return json_response(json!({
                "user_badges": [],
                "user": {
                    "id": user.id,
                    "username": user.username,
                    "name": "",
                    "created_at": instant_text(user.created_at),
                    "trust_level": user.trust,
                    "suspended_till": null,
                },
            }));
        }
        let segments: Vec<&str> = raw_path.trim_start_matches('/').split('/').collect();
        if segments.len() == 2 && segments[0] == "t" {
            let Some(id) = segments[1]
                .strip_suffix(".json")
                .and_then(|id| id.parse::<i64>().ok())
            else {
                return not_found();
            };
            let state = self.state();
            let ids: Vec<i64> = state
                .posts
                .iter()
                .filter_map(|post| post["id"].as_i64())
                .collect();
            let first: Vec<Value> = state.posts.iter().take(20).cloned().collect();
            return json_response(json!({
                "id": id,
                "posts_count": ids.len(),
                "chunk_size": 20,
                "post_stream": { "posts": first, "stream": ids },
            }));
        }
        if segments.len() == 3 && segments[0] == "t" && segments[2] == "posts.json" {
            self.post_requests.fetch_add(1, Ordering::SeqCst);
            let query: Vec<(String, String)> = request.url.query_pairs().into_owned().collect();
            // The Kotlin client always asks for the raw text; anything else would lose phrase matching.
            if !query
                .iter()
                .any(|(key, value)| key == "include_raw" && value == "true")
            {
                return ResponseTemplate::new(400)
                    .set_body_string("{\"errors\":[\"include_raw missing\"]}");
            }
            let ids: Vec<i64> = query
                .iter()
                .filter(|(key, _)| key == "post_ids[]")
                .filter_map(|(_, value)| value.parse::<i64>().ok())
                .collect();
            let state = self.state();
            let posts: Vec<Value> = state
                .posts
                .iter()
                .filter(|post| post["id"].as_i64().is_some_and(|id| ids.contains(&id)))
                .cloned()
                .collect();
            return json_response(json!({ "id": TOPIC, "post_stream": { "posts": posts } }));
        }
        not_found()
    }
}

fn json_response(value: Value) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "application/json")
        .set_body_json(value)
}

fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_string("{\"errors\":[\"not found\"]}")
}

struct ForumEndpoint(Arc<FakeForum>);

impl wiremock::Respond for ForumEndpoint {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        self.0.respond(request)
    }
}

async fn mount_forum(forum: &Arc<FakeForum>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ForumEndpoint(forum.clone()))
        .mount(&server)
        .await;
    server
}

/// An OpenAI-compatible upstream that reports a usage of 15 tokens.
async fn mount_upstream() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/chat/completions"))
        .respond_with(json_response(json!({
            "id": "c1",
            "object": "chat.completion",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "你好" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 },
        })))
        .mount(&server)
        .await;
    server
}

fn claim_config(forum_url: &str) -> ClaimConfig {
    let expires = ClaimConfig::parse_time("2026-11-12 21:00", 480).expect("the end of the class");
    ClaimConfig {
        forum_url: forum_url.to_string(),
        forum_timeout_seconds: 3,
        ..ClaimConfig::new(TOPIC, expires)
    }
}

struct Harness {
    server: BoundServer,
    base: String,
    client: reqwest::Client,
    file: PathBuf,
    now: Arc<AtomicI64>,
    logs: Arc<Mutex<Vec<String>>>,
}

impl Harness {
    async fn start(
        claim: Option<ClaimConfig>,
        upstream: &str,
        file: PathBuf,
        access_code: Option<&str>,
    ) -> Self {
        let now = Arc::new(AtomicI64::new(start_millis()));
        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let class = ClassOptions {
            reload_millis: 0,
            ..ClassOptions::new(
                UPSTREAM_KEY,
                vec![("xiaoman-class".to_string(), "doubao-real".to_string())],
                upstream.to_string(),
                file.clone(),
            )
        };
        let options = WebOptions {
            port: 0,
            allow_private: true,
            access_code: access_code.map(str::to_string),
            class: Some(class),
            claim,
            ..Default::default()
        };
        let clock: Clock = {
            let now = now.clone();
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let logger = {
            let logs = logs.clone();
            Arc::new(move |line: &str| {
                logs.lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(line.to_string())
            })
        };
        let server = WebServer::new(options)
            .with_clock(clock)
            .with_logger(logger)
            .bind()
            .await
            .expect("the server binds");
        let base = format!("http://127.0.0.1:{}", server.port());
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("client");
        Self {
            server,
            base,
            client,
            file,
            now,
            logs,
        }
    }

    fn now(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }

    fn advance(&self, millis: i64) {
        self.now.fetch_add(millis, Ordering::SeqCst);
    }

    fn set_now(&self, millis: i64) {
        self.now.store(millis, Ordering::SeqCst);
    }

    fn logs(&self) -> Vec<String> {
        self.logs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    async fn call(&self, path: &str, body: Option<Value>, headers: &[(&str, &str)]) -> Reply {
        let mut request = match &body {
            Some(body) => self.client.post(format!("{}{path}", self.base)).json(body),
            None => self.client.get(format!("{}{path}", self.base)),
        };
        for (key, value) in headers {
            request = request.header(*key, *value);
        }
        let response = request
            .send()
            .await
            .expect("the request reaches the server");
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let retry_after = header("retry-after");
        let cache_control = header("cache-control");
        let text = response.text().await.unwrap_or_default();
        let body = serde_json::from_str(&text).unwrap_or(Value::Null);
        Reply {
            status,
            text,
            body,
            retry_after,
            cache_control,
        }
    }

    async fn api_info(&self) -> Value {
        let reply = self.call("/api/info", None, &[]).await;
        assert_eq!(reply.status, 200, "{}", reply.text);
        reply.json().clone()
    }

    async fn info(&self) -> Value {
        self.api_info().await["claim"].clone()
    }

    async fn start_claim(&self, username: &str, ip: Option<&str>) -> Reply {
        let headers: Vec<(&str, &str)> = ip
            .map(|ip| vec![("x-forwarded-for", ip)])
            .unwrap_or_default();
        self.call(
            "/api/claim/start",
            Some(json!({ "username": username })),
            &headers,
        )
        .await
    }

    /// Each check comes a few seconds later than the last, past the server's short topic cache.
    async fn verify(&self, challenge_id: &str) -> Reply {
        self.advance(4_000);
        self.call(
            "/api/claim/verify",
            Some(json!({ "challengeId": challenge_id })),
            &[],
        )
        .await
    }

    /// The full flow for `user`; returns the plaintext token.
    async fn claim(&self, forum: &Arc<FakeForum>, user: &FakeUser, typed: &str) -> String {
        let started = self.start_claim(typed, None).await;
        assert_eq!(started.status, 200, "{}", started.text);
        forum.reply(
            user,
            &format!("老师好，{} 谢谢", started.str("phrase")),
            self.now(),
        );
        let done = self.verify(started.str("challengeId")).await;
        assert_eq!(done.status, 200, "{}", done.text);
        done.str("token").to_string()
    }

    fn tokens(&self) -> Vec<ClassToken> {
        let clock: Clock = {
            let now = self.now.clone();
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        ClassTokenStore::new(self.file.clone(), 480, clock, 0).all()
    }

    fn admin(&self, args: &[&str]) -> (String, String) {
        let mut argv: Vec<String> = args.iter().map(|argument| argument.to_string()).collect();
        argv.push("--file".to_string());
        argv.push(self.file.display().to_string());
        let clock: Clock = {
            let now = self.now.clone();
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = xiaoman_web::tokens_admin::run(&argv, &mut out, &mut err, &|_| None, clock);
        assert_eq!(
            code,
            0,
            "admin {args:?} failed: {}",
            String::from_utf8_lossy(&err)
        );
        (
            String::from_utf8(out).expect("utf-8 stdout"),
            String::from_utf8(err).expect("utf-8 stderr"),
        )
    }

    async fn new_session(&self) -> String {
        let reply = self.call("/api/session", Some(json!({})), &[]).await;
        assert_eq!(reply.status, 200, "{}", reply.text);
        reply.str("sessionId").to_string()
    }

    /// Sends one chat turn and returns the HTTP status and the decoded SSE events.
    async fn chat(&self, session: &str, body: &Value) -> (u16, Vec<Value>) {
        let response = self
            .client
            .post(format!("{}/api/chat", self.base))
            .header("x-xiaoman-session", session)
            .json(body)
            .send()
            .await
            .expect("chat request");
        let status = response.status().as_u16();
        let text = tokio::time::timeout(Duration::from_secs(10), response.text())
            .await
            .expect("the stream ends")
            .unwrap_or_default();
        let events = text
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|data| serde_json::from_str::<Value>(data).ok())
            .collect();
        (status, events)
    }

    async fn shutdown(self) {
        self.server.shutdown().await;
    }
}

struct Reply {
    status: u16,
    text: String,
    body: Value,
    retry_after: Option<String>,
    cache_control: Option<String>,
}

impl Reply {
    fn json(&self) -> &Value {
        &self.body
    }

    fn str(&self, key: &str) -> &str {
        self.body
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    fn code(&self) -> &str {
        self.str("code")
    }
}

fn types(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .map(|event| event["type"].as_str().unwrap_or_default())
        .collect()
}

fn temp_file() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = dir.path().join("class-tokens.json");
    (dir, file)
}

/// A port nothing listens on, for the "forum is down" case.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("local address").port();
    drop(listener);
    port
}

fn assert_no_token_in_logs(logs: &[String], plaintext: &[&str]) {
    for plain in plaintext {
        for line in logs {
            assert!(!line.contains(plain), "a log leaked the token: {line}");
        }
    }
}

#[tokio::test]
async fn claim_issues_one_token_with_configured_expiry_and_limits_that_works_on_a_chat_turn() {
    let (_dir, file) = temp_file();
    let forum_state = FakeForum::new();
    let forum = mount_forum(&forum_state).await;
    let upstream = mount_upstream().await;
    let alice = forum_state.user(101, "Alice", start_millis() - 86_400_000, 1);
    forum_state.filler(5, start_millis() - 86_400_000);
    let harness = Harness::start(
        Some(claim_config(&forum.uri())),
        &format!("{}/api/v3", upstream.uri()),
        file.clone(),
        None,
    )
    .await;

    assert_eq!(harness.info().await["state"], "open");
    assert_eq!(harness.info().await["remaining"], 50);
    assert_eq!(
        harness.api_info().await["classTokens"],
        false,
        "the token file does not exist yet"
    );

    let started = harness.start_claim("@alice", None).await;
    assert_eq!(started.status, 200, "{}", started.text);
    let phrase = started.str("phrase").to_string();
    let code = phrase.split_once(' ').expect("a phrase and its code").1;
    assert_eq!(phrase.split_once(' ').expect("a phrase").0, "领取小满令牌");
    assert_eq!(code.len(), 6);
    assert!(
        code.chars()
            .all(|character| "23456789ABCDEFGHJKMNPQRSTUVWXYZ".contains(character)),
        "{phrase}"
    );
    assert_eq!(
        started.str("username"),
        "Alice",
        "the forum's spelling is used"
    );
    assert_eq!(
        started.str("topicUrl"),
        format!("{}/t/{TOPIC}", forum.uri())
    );
    assert_eq!(started.json()["expiresInSeconds"], 600);
    assert_eq!(started.cache_control.as_deref(), Some("no-store"));

    // Typed on a phone: extra spaces, full-width letters, lower case, inside other text.
    let typed: String = code
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_lowercase() {
                char::from_u32(character as u32 - 'a' as u32 + 0xFF41).expect("full-width letter")
            } else {
                character
            }
        })
        .collect();
    let at = harness.now();
    forum_state.reply(&alice, &format!("收到！领取 小满令牌\u{3000}{typed}"), at);
    let done = harness.verify(started.str("challengeId")).await;
    assert_eq!(done.status, 200, "{}", done.text);
    let token = done.str("token").to_string();
    assert!(ClassTokenCodec::looks_like_token(&token), "{token}");
    assert_eq!(done.str("expiresAt"), "2026-11-12 21:00");
    assert_eq!(done.str("model"), "xiaoman-class");
    assert_eq!(done.cache_control.as_deref(), Some("no-store"));

    let claimed_at = harness.now();
    let stored = harness.tokens();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].label, "论坛:Alice");
    assert_eq!(
        stored[0].expires_at,
        Some(ClaimConfig::parse_time("2026-11-12 21:00", 480).unwrap())
    );
    assert_eq!(stored[0].total_limit, Some(1_000_000));
    assert_eq!(stored[0].daily_limit, Some(1_000_000));
    assert_eq!(
        stored[0].claim,
        Some(ForumClaim {
            user_id: 101,
            username: "Alice".to_string(),
            claimed_at,
            reset: false
        })
    );
    assert_eq!(harness.info().await["remaining"], 49);

    // The same challenge cannot be used twice.
    assert_eq!(
        harness.verify(started.str("challengeId")).await.code(),
        "challenge_expired"
    );

    // The claimed token works for a chat turn and is metered against the instructor's upstream.
    let session = harness.new_session().await;
    let (status, events) = harness
        .chat(
            &session,
            &json!({
                "message": "你好",
                "model": { "classToken": token },
                "mcp": { "keys": {}, "custom": [] },
            }),
        )
        .await;
    assert_eq!(status, 200);
    assert!(types(&events).contains(&"reply"), "{events:?}");
    assert_eq!(harness.api_info().await["classTokens"], true);
    let prefix: String = token.chars().take(6).collect();
    assert!(
        harness
            .logs()
            .iter()
            .any(|line| line == &format!("自助领取 论坛:Alice({prefix}) 已发放")),
        "{:?}",
        harness.logs()
    );
    assert_no_token_in_logs(&harness.logs(), &[&token]);

    harness.shutdown().await;
    let after = ClassTokenStore::new(file, 480, Arc::new(xiaoman_api::now_millis), 0).all();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].used_total, 15,
        "the upstream reported 10 + 5 tokens"
    );
}

#[tokio::test]
async fn wrong_expired_or_foreign_phrases_do_not_issue() {
    let (_dir, file) = temp_file();
    let forum_state = FakeForum::new();
    let forum = mount_forum(&forum_state).await;
    let alice = forum_state.user(101, "alice", start_millis() - 86_400_000, 1);
    let bob = forum_state.user(102, "bob", start_millis() - 86_400_000, 1);
    let harness = Harness::start(
        Some(claim_config(&forum.uri())),
        "http://127.0.0.1:9/api/v3",
        file.clone(),
        None,
    )
    .await;

    let started = harness.start_claim("alice", None).await;
    let id = started.str("challengeId").to_string();
    let phrase = started.str("phrase").to_string();
    assert_eq!(harness.verify(&id).await.code(), "reply_not_found");
    forum_state.reply(&alice, "领取小满令牌 AAAAAA", harness.now());
    let not_found = harness.verify(&id).await;
    assert_eq!(not_found.status, 404);
    assert_eq!(not_found.code(), "reply_not_found");
    // Bob posts Alice's phrase: found, but by the wrong account.
    forum_state.reply(&bob, &phrase, harness.now());
    let wrong = harness.verify(&id).await;
    assert_eq!(wrong.code(), "reply_wrong_user");
    assert!(wrong.str("error").contains("bob"));
    // Alice quoting Bob's post does not count either: only her own words are checked.
    forum_state.reply_full(
        &alice,
        &format!("[quote=\"bob, post:3\"]\n{phrase}\n[/quote]\n顶"),
        &format!("<aside class=\"quote no-group\"><blockquote><p>{phrase}</p></blockquote></aside><p>顶</p>"),
        harness.now(),
        1,
        false,
    );
    assert_eq!(harness.verify(&id).await.code(), "reply_wrong_user");
    // Hidden or moderator posts are ignored.
    forum_state.reply_full(&alice, &phrase, "<p>x</p>", harness.now(), 1, true);
    assert_eq!(harness.verify(&id).await.code(), "reply_wrong_user");

    harness.advance(11 * 60_000);
    forum_state.reply(&alice, &phrase, harness.now());
    let late = harness.verify(&id).await;
    assert_eq!(late.status, 410);
    assert_eq!(late.code(), "challenge_expired");
    assert_eq!(harness.verify("missing").await.code(), "challenge_expired");
    assert!(!file.exists(), "no token was issued");
}

#[tokio::test]
async fn one_token_per_forum_account_and_a_revoked_one_is_not_reissued_until_reset() {
    let (_dir, file) = temp_file();
    let forum_state = FakeForum::new();
    let forum = mount_forum(&forum_state).await;
    let alice = forum_state.user(101, "Alice", start_millis() - 86_400_000, 1);
    let config = ClaimConfig {
        per_ip_starts_per_minute: 20,
        ..claim_config(&forum.uri())
    };
    let harness = Harness::start(
        Some(config),
        "http://127.0.0.1:9/api/v3",
        file.clone(),
        None,
    )
    .await;

    // Two challenges started before the first claim: the second one must not issue another token.
    let first = harness.start_claim("alice", None).await;
    let second = harness.start_claim("ALICE", None).await;
    forum_state.reply(&alice, first.str("phrase"), harness.now());
    forum_state.reply(&alice, second.str("phrase"), harness.now());
    let token = harness.verify(first.str("challengeId")).await;
    assert_eq!(token.status, 200, "{}", token.text);
    let token = token.str("token").to_string();
    let again = harness.verify(second.str("challengeId")).await;
    assert_eq!(again.status, 409);
    assert_eq!(again.code(), "already_claimed");
    assert_eq!(again.str("prefix"), token[..6].to_string());
    let restart = harness.start_claim("aLiCe", None).await;
    assert_eq!(restart.code(), "already_claimed");
    assert!(
        restart.str("error").contains(&token[..6]),
        "{}",
        restart.text
    );
    assert_eq!(harness.tokens().len(), 1);

    harness.admin(&["revoke", "论坛:Alice"]);
    let revoked = harness.start_claim("alice", None).await;
    assert_eq!(revoked.code(), "already_claimed");
    assert_eq!(revoked.json()["revoked"], true);
    assert!(revoked.str("error").contains("作废"));
    harness.admin(&["restore", "论坛:Alice"]);
    assert_eq!(
        harness.start_claim("alice", None).await.code(),
        "already_claimed"
    );

    let (_out, reset_message) = harness.admin(&["reset-claim", "alice"]);
    assert!(
        reset_message.contains(&format!("已重置 论坛:Alice({})", &token[..6])),
        "{reset_message}"
    );
    let new_token = harness.claim(&forum_state, &alice, "alice").await;
    assert_ne!(new_token, token);
    let all = harness.tokens();
    assert_eq!(all.len(), 2);
    assert!(
        all.iter()
            .find(|stored| stored.prefix == token[..6])
            .is_some_and(|stored| stored.revoked),
        "reset revokes the old token"
    );
    assert_eq!(active_claims(&all).len(), 1);
    let (list, _) = harness.admin(&["claims"]);
    assert!(
        list.lines()
            .any(|line| line.starts_with(&format!("Alice\t101\t{}\t已重置", &token[..6]))),
        "{list}"
    );
    assert!(
        list.lines().any(|line| line.starts_with(&format!(
            "Alice\t101\t{}\t可用\t0/1000000\t0/1000000",
            &new_token[..6]
        ))),
        "{list}"
    );
    assert!(list.contains("已领取 1 份（另有 1 条已重置）"), "{list}");
    assert_no_token_in_logs(&harness.logs(), &[&token, &new_token]);
}

#[tokio::test]
async fn reset_claim_needs_an_existing_claim() {
    let (_dir, file) = temp_file();
    let harness = Harness::start(None, "http://127.0.0.1:9/api/v3", file, None).await;
    let mut argv: Vec<String> = vec![
        "reset-claim".to_string(),
        "nobody".to_string(),
        "--file".to_string(),
    ];
    argv.push(harness.file.display().to_string());
    let clock: Clock = {
        let now = harness.now.clone();
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = xiaoman_web::tokens_admin::run(&argv, &mut out, &mut err, &|_| None, clock);
    assert_eq!(code, 2);
    assert!(String::from_utf8_lossy(&err).contains("找不到「nobody」"));
}

#[tokio::test]
async fn the_total_limit_and_expiry_close_the_claim() {
    let (_dir, file) = temp_file();
    let forum_state = FakeForum::new();
    let forum = mount_forum(&forum_state).await;
    let users: Vec<FakeUser> = (1..=3)
        .map(|index| {
            forum_state.user(
                200 + index,
                &format!("user{index}"),
                start_millis() - 86_400_000,
                1,
            )
        })
        .collect();
    let config = ClaimConfig {
        max: 2,
        ..claim_config(&forum.uri())
    };
    let harness = Harness::start(
        Some(config),
        "http://127.0.0.1:9/api/v3",
        file.clone(),
        None,
    )
    .await;

    // user3 starts while there is room, but the limit is checked again when issuing.
    let late = harness.start_claim("user3", None).await;
    forum_state.reply(&users[2], late.str("phrase"), harness.now());
    harness.claim(&forum_state, &users[0], "user1").await;
    harness.claim(&forum_state, &users[1], "user2").await;
    assert_eq!(harness.info().await["state"], "full");
    assert_eq!(harness.info().await["remaining"], 0);
    assert_eq!(
        harness.start_claim("user3", None).await.code(),
        "claim_full"
    );
    assert_eq!(
        harness.verify(late.str("challengeId")).await.code(),
        "claim_full"
    );
    // Resetting an account frees its slot.
    harness.admin(&["reset-claim", "user1"]);
    assert_eq!(harness.info().await["state"], "open");
    assert_eq!(harness.verify(late.str("challengeId")).await.status, 200);
    assert_eq!(harness.info().await["state"], "full");

    harness.set_now(ClaimConfig::parse_time("2026-11-12 21:00", 480).unwrap());
    assert_eq!(harness.info().await["state"], "expired");
    let expired = harness.start_claim("user1", None).await;
    assert_eq!(expired.status, 410);
    assert_eq!(expired.code(), "claim_expired");
    assert!(expired.str("error").contains("2026-11-12 21:00"));
}

#[tokio::test]
async fn account_age_and_trust_level_are_checked() {
    let (_dir, file) = temp_file();
    let forum_state = FakeForum::new();
    let forum = mount_forum(&forum_state).await;
    forum_state.user(
        301,
        "newbie",
        chrono::DateTime::parse_from_rfc3339("2026-11-05T00:00:00Z")
            .unwrap()
            .timestamp_millis(),
        1,
    );
    let old = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    forum_state.user(302, "visitor", old, 0);
    forum_state.user(303, "regular", old, 2);
    let config = ClaimConfig {
        created_before: ClaimConfig::parse_time("2026-11-01", 480),
        min_trust: 1,
        ..claim_config(&forum.uri())
    };
    let harness = Harness::start(Some(config), "http://127.0.0.1:9/api/v3", file, None).await;

    let young = harness.start_claim("newbie", None).await;
    assert_eq!(young.status, 403);
    assert_eq!(young.code(), "account_too_new");
    assert!(
        young.str("error").contains("2026-11-01 00:00"),
        "{}",
        young.text
    );
    assert_eq!(
        harness.start_claim("visitor", None).await.code(),
        "trust_too_low"
    );
    assert_eq!(harness.start_claim("regular", None).await.status, 200);
    let ghost = harness.start_claim("ghost", None).await;
    assert_eq!(ghost.status, 404);
    assert_eq!(ghost.code(), "user_not_found");
}

#[tokio::test]
async fn invalid_usernames_are_rejected_before_asking_the_forum() {
    let (_dir, file) = temp_file();
    let forum_state = FakeForum::new();
    let forum = mount_forum(&forum_state).await;
    let harness = Harness::start(
        Some(claim_config(&forum.uri())),
        "http://127.0.0.1:9/api/v3",
        file,
        None,
    )
    .await;
    let too_long = "a".repeat(61);
    for bad in ["", "a b", "../admin", "x/y", too_long.as_str(), "名字?"] {
        assert_eq!(
            harness.start_claim(bad, None).await.code(),
            "invalid_username",
            "{bad}"
        );
    }
    assert_eq!(
        forum_state.requests.load(Ordering::SeqCst),
        0,
        "no username reached the forum"
    );
    forum_state.user(401, "汤圆", start_millis() - 86_400_000, 1);
    assert_eq!(
        harness.start_claim("汤圆", None).await.status,
        200,
        "Discourse allows Unicode usernames"
    );
}

#[tokio::test]
async fn forum_failures_never_issue_tokens() {
    let (_dir, file) = temp_file();
    let forum_state = FakeForum::new();
    let forum = mount_forum(&forum_state).await;
    let alice = forum_state.user(101, "alice", start_millis() - 86_400_000, 1);
    let harness = Harness::start(
        Some(claim_config(&forum.uri())),
        "http://127.0.0.1:9/api/v3",
        file.clone(),
        None,
    )
    .await;

    let started = harness.start_claim("alice", None).await;
    forum_state.reply(&alice, started.str("phrase"), harness.now());
    forum_state.failing.store(true, Ordering::SeqCst);
    let down = harness.verify(started.str("challengeId")).await;
    assert_eq!(down.status, 503);
    assert_eq!(down.code(), "forum_unavailable");
    assert_eq!(
        harness.start_claim("alice", None).await.code(),
        "forum_unavailable"
    );
    assert!(!file.exists());
    forum_state.failing.store(false, Ordering::SeqCst);
    assert_eq!(
        harness.verify(started.str("challengeId")).await.status,
        200,
        "the challenge survives an outage"
    );
    let logs = harness.logs();
    harness.shutdown().await;

    let (_dir2, file2) = temp_file();
    let port = closed_port();
    let unreachable = Harness::start(
        Some(claim_config(&format!("http://127.0.0.1:{port}"))),
        "http://127.0.0.1:9/api/v3",
        file2,
        None,
    )
    .await;
    let reply = unreachable.start_claim("alice", None).await;
    assert_eq!(reply.status, 503);
    assert_eq!(reply.code(), "forum_unavailable");
    assert!(reply.str("error").contains("稍后重试"));

    let stored =
        ClassTokenStore::new(file.clone(), 480, Arc::new(xiaoman_api::now_millis), 0).all();
    assert_eq!(stored.len(), 1);
    assert!(
        logs.iter().any(|line| line.contains("论坛请求失败")),
        "{logs:?}"
    );
}

#[tokio::test]
async fn starts_are_rate_limited_per_client_ip() {
    let (_dir, file) = temp_file();
    let forum_state = FakeForum::new();
    let forum = mount_forum(&forum_state).await;
    forum_state.user(101, "alice", start_millis() - 86_400_000, 1);
    let config = ClaimConfig {
        per_ip_starts_per_minute: 3,
        ..claim_config(&forum.uri())
    };
    let harness = Harness::start(Some(config), "http://127.0.0.1:9/api/v3", file, None).await;

    for _ in 0..3 {
        assert_eq!(
            harness
                .start_claim("alice", Some("203.0.113.5"))
                .await
                .status,
            200
        );
    }
    let limited = harness.start_claim("alice", Some("203.0.113.5")).await;
    assert_eq!(limited.status, 429);
    assert_eq!(limited.code(), "rate_limited");
    assert!(limited.retry_after.is_some());
    // Behind Caddy the client is the last X-Forwarded-For entry; a spoofed first entry does not help.
    assert_eq!(
        harness
            .start_claim("alice", Some("1.2.3.4, 203.0.113.5"))
            .await
            .status,
        429
    );
    assert_eq!(
        harness
            .start_claim("alice", Some("198.51.100.7"))
            .await
            .status,
        200
    );
    harness.advance(61_000);
    assert_eq!(
        harness
            .start_claim("alice", Some("203.0.113.5"))
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn long_topics_are_read_newest_first_in_batches() {
    let (_dir, file) = temp_file();
    let forum_state = FakeForum::new();
    let forum = mount_forum(&forum_state).await;
    let alice = forum_state.user(101, "alice", start_millis() - 86_400_000, 1);
    forum_state.filler(400, start_millis() - 86_400_000);
    let harness = Harness::start(
        Some(claim_config(&forum.uri())),
        "http://127.0.0.1:9/api/v3",
        file,
        None,
    )
    .await;

    let started = harness.start_claim("alice", None).await;
    forum_state.reply(&alice, started.str("phrase"), harness.now());
    forum_state.filler(120, harness.now()); // newer replies after hers
    assert_eq!(harness.verify(started.str("challengeId")).await.status, 200);
    assert_eq!(
        forum_state.post_requests.load(Ordering::SeqCst),
        3,
        "120 newer posts + hers = 3 batches of 50"
    );

    // A phrase not posted: reading stops once the posts are older than the phrase, not at 300.
    forum_state.user(102, "bob", start_millis() - 86_400_000, 1);
    let second = harness.start_claim("bob", None).await;
    forum_state.post_requests.store(0, Ordering::SeqCst);
    assert_eq!(
        harness.verify(second.str("challengeId")).await.code(),
        "reply_not_found"
    );
    assert!(
        forum_state.post_requests.load(Ordering::SeqCst) <= 1,
        "cached recent posts; {} requests",
        forum_state.post_requests.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn claiming_works_without_the_access_code_and_the_page_is_served() {
    let (_dir, file) = temp_file();
    let forum_state = FakeForum::new();
    let forum = mount_forum(&forum_state).await;
    forum_state.user(101, "alice", start_millis() - 86_400_000, 1);
    let harness = Harness::start(
        Some(claim_config(&forum.uri())),
        "http://127.0.0.1:9/api/v3",
        file,
        Some("classcode1"),
    )
    .await;

    assert_eq!(
        harness
            .call("/api/session", Some(json!({})), &[])
            .await
            .status,
        401
    );
    assert_eq!(harness.start_claim("alice", None).await.status, 200);
    let page = harness.call("/claim", None, &[]).await;
    assert_eq!(page.status, 200);
    assert!(page.text.contains("领取课堂令牌"));
    assert_eq!(harness.call("/claim.js", None, &[]).await.status, 200);
}

#[tokio::test]
async fn info_shows_off_without_a_claim_config() {
    let server = WebServer::new(WebOptions {
        port: 0,
        allow_private: true,
        ..Default::default()
    })
    .with_logger(Arc::new(|_line: &str| {}))
    .bind()
    .await
    .expect("bind");
    let base = format!("http://127.0.0.1:{}", server.port());
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client");
    let info: Value = client
        .get(format!("{base}/api/info"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["claim"]["state"], "off");
    let reply = client
        .post(format!("{base}/api/claim/start"))
        .json(&json!({ "username": "alice" }))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 404);
    let body: Value = reply.json().await.unwrap();
    assert_eq!(body["error"], "自助领取没有开启");
}

#[test]
fn config_from_the_environment() {
    let none = |_: &str| None;
    assert!(
        ClaimConfig::from_env(&none, 480, start_millis())
            .unwrap()
            .0
            .is_none()
    );

    let vars = [
        (
            "XIAOMAN_CLAIM_TOPIC_ID",
            "https://forum.trae.cn/t/xiaoman-claim/465300/3",
        ),
        ("XIAOMAN_CLAIM_EXPIRES", "2026-11-12 21:00"),
        ("XIAOMAN_CLAIM_MIN_CREATED_BEFORE", "2026-11-01"),
        ("XIAOMAN_CLAIM_MIN_TRUST", "1"),
        ("XIAOMAN_CLAIM_TOTAL_TOKENS", "2_000_000"),
    ];
    let env = |name: &str| {
        vars.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    };
    let (config, status) = ClaimConfig::from_env(&env, 480, start_millis()).unwrap();
    let config = config.expect("claiming is configured");
    assert_eq!(status, "已开启");
    assert_eq!(config.topic_id, 465300);
    assert_eq!(
        config.expires_at,
        ClaimConfig::parse_time("2026-11-12 21:00", 480).unwrap()
    );
    assert_eq!(config.max, 50);
    assert_eq!(config.total_tokens, 2_000_000);
    assert_eq!(
        config.daily_tokens, 2_000_000,
        "daily defaults to the total"
    );
    assert_eq!(
        config.created_before,
        Some(
            chrono::DateTime::parse_from_rfc3339("2026-10-31T16:00:00Z")
                .unwrap()
                .timestamp_millis()
        )
    );
    assert_eq!(config.min_trust, 1);
    assert_eq!(config.topic_url(), "https://forum.trae.cn/t/465300");

    let numeric = |name: &str| match name {
        "XIAOMAN_CLAIM_TOPIC_ID" => Some("465300".to_string()),
        "XIAOMAN_CLAIM_EXPIRES" => Some("2026-11-12T21:00:00+08:00".to_string()),
        _ => None,
    };
    assert_eq!(
        ClaimConfig::from_env(&numeric, 480, start_millis())
            .unwrap()
            .0
            .unwrap()
            .topic_id,
        465300
    );

    let broken =
        |topic: &'static str, expires: Option<&'static str>, trust: Option<&'static str>| {
            move |name: &str| match name {
                "XIAOMAN_CLAIM_TOPIC_ID" => Some(topic.to_string()),
                "XIAOMAN_CLAIM_EXPIRES" => expires.map(str::to_string),
                "XIAOMAN_CLAIM_MIN_TRUST" => trust.map(str::to_string),
                _ => None,
            }
        };
    assert!(ClaimConfig::from_env(&broken("465300", None, None), 480, start_millis()).is_err());
    assert!(
        ClaimConfig::from_env(
            &broken("abc", Some("2026-11-12 21:00"), None),
            480,
            start_millis()
        )
        .is_err()
    );
    assert!(ClaimConfig::from_env(&broken("1", Some("明天"), None), 480, start_millis()).is_err());
    assert!(
        ClaimConfig::from_env(
            &broken("1", Some("2026-11-12 21:00"), Some("9")),
            480,
            start_millis()
        )
        .is_err()
    );
}

#[tokio::test]
async fn a_chat_turn_reports_the_kotlin_reasons_for_a_bad_class_token() {
    let (_dir, file) = temp_file();
    let harness = Harness::start(None, "http://127.0.0.1:9/api/v3", file.clone(), None).await;
    let session = harness.new_session().await;
    let body = |token: &str| {
        json!({
            "message": "你好",
            "model": { "classToken": token },
            "mcp": { "keys": {}, "custom": [] },
        })
    };
    let message = |events: &[Value]| {
        events[0]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    // No token file yet: class-token mode is off, exactly as the Kotlin server says.
    let (status, events) = harness
        .chat(&session, &body("xm-aaaaaaaaaaaaaaaaaaaa"))
        .await;
    assert_eq!(status, 200);
    assert_eq!(types(&events), vec!["error", "done"]);
    assert_eq!(
        message(&events),
        "服务器没有开启课堂令牌，请改用自己的 API Key"
    );

    let (out, _) = harness.admin(&["generate", "1"]);
    let plain = out
        .trim()
        .split_once('\t')
        .expect("label and token")
        .1
        .trim()
        .to_string();
    let (_, events) = harness
        .chat(&session, &body("xm-bbbbbbbbbbbbbbbbbbbb"))
        .await;
    assert_eq!(message(&events), "课堂令牌无效");

    harness.admin(&["revoke", "学员 01"]);
    let (_, events) = harness.chat(&session, &body(&plain)).await;
    assert_eq!(message(&events), "课堂令牌已作废");

    harness.admin(&["restore", "学员 01"]);
    harness.admin(&["limit", "学员 01", "--expires", "2020-01-01 00:00"]);
    let (_, events) = harness.chat(&session, &body(&plain)).await;
    assert_eq!(message(&events), "课堂令牌已过期");
}

#[tokio::test]
async fn claim_records_survive_other_admin_commands() {
    let (_dir, file) = temp_file();
    let forum_state = FakeForum::new();
    let forum = mount_forum(&forum_state).await;
    let alice = forum_state.user(101, "alice", start_millis() - 86_400_000, 1);
    let harness = Harness::start(
        Some(claim_config(&forum.uri())),
        "http://127.0.0.1:9/api/v3",
        file,
        None,
    )
    .await;
    harness.claim(&forum_state, &alice, "alice").await;

    harness.admin(&["generate", "2"]);
    harness.admin(&[
        "limit",
        "--all",
        "--expires",
        "2026-11-13 21:00",
        "--total",
        "2000000",
    ]);
    let stored: Vec<ClassToken> = harness
        .tokens()
        .into_iter()
        .filter(|token| token.claim.is_some())
        .collect();
    assert_eq!(stored.len(), 1);
    let claim = stored[0]
        .claim
        .clone()
        .expect("the claimed token keeps its claim");
    assert_eq!(claim.user_id, 101);
    assert_eq!(claim.username, "alice");
    assert!(!claim.reset);
    assert_eq!(stored[0].total_limit, Some(2_000_000));
    assert_eq!(
        stored[0].expires_at,
        ClaimConfig::parse_time("2026-11-13 21:00", 480)
    );
    assert_eq!(harness.tokens().len(), 3);
}
