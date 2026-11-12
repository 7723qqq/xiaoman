//! HTTP and SSE adapter; credentials and conversation state stay in memory. Long-term memory lives
//! in the browser and comes with each chat request ([`crate::BrowserMemory`]); nothing is written
//! to disk.

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use futures::Stream;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use xiaoman_api::{Agent, AgentEvent, AgentListener};
use xiaoman_mcp::{McpCatalog, McpPlatform};

use crate::browser_memory::BrowserMemory;
use crate::claims::{ClaimReply, ClaimService};
use crate::class_gateway::ClassGateway;
use crate::local_launch;
use crate::mcp_provider::{StdioLimiter, TransportFactory, build_provider};
use crate::mcp_selection::{SelectionOptions, build_mcp_config};
use crate::memory::Clock;
use crate::options::WebOptions;
use crate::problem::{Problem, json_response};
use crate::safe_dns::is_public_url;
use crate::session::{
    AssembleInputs, ChatSkillRepository, LlmFactory, SessionStop, SessionStore, WebSession,
    llm_factory,
};
use crate::static_files::StaticFiles;

/// Bodies larger than this are rejected before anything is parsed.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// The only API route names that may appear in the log; paths can carry credentials.
pub const API_PATHS: [&str; 8] = [
    "/api/info",
    "/api/catalog",
    "/api/session",
    "/api/chat",
    "/api/answer",
    "/api/mcp/test",
    "/api/claim/start",
    "/api/claim/verify",
];

const HEADER_ACCESS: &str = "x-xiaoman-access";
const HEADER_SESSION: &str = "x-xiaoman-session";

pub type Logger = Arc<dyn Fn(&str) + Send + Sync>;

pub struct AppState {
    options: Arc<WebOptions>,
    llm: LlmFactory,
    skills: Arc<dyn xiaoman_api::SkillRepository>,
    clock: Clock,
    logger: Logger,
    catalog: &'static McpCatalog,
    transports: TransportFactory,
    sessions: Mutex<SessionStore>,
    static_files: StaticFiles,
    /// Class-token mode; `None` until `XIAOMAN_CLASS_*` is configured.
    gateway: Option<Arc<ClassGateway>>,
    /// Self-service claiming; needs the gateway and `XIAOMAN_CLAIM_*`.
    claims: Option<Arc<ClaimService>>,
}

impl AppState {
    /// The settings this server was built with, for the local-mode announcements.
    pub(crate) fn options(&self) -> &WebOptions {
        &self.options
    }

    fn session(&self, headers: &HeaderMap) -> Option<Arc<WebSession>> {
        let id = headers.get(HEADER_SESSION)?.to_str().ok()?;
        self.sessions.lock().unwrap().session(id)
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        let Some(code) = self
            .options
            .access_code
            .as_deref()
            .filter(|code| !code.is_empty())
        else {
            return true;
        };
        let provided = headers
            .get(HEADER_ACCESS)
            .map(|value| value.as_bytes().to_vec())
            .unwrap_or_default();
        constant_time_eq(code.as_bytes(), &provided)
    }
}

fn constant_time_eq(expected: &[u8], provided: &[u8]) -> bool {
    if expected.len() != provided.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in expected.iter().zip(provided) {
        difference |= left ^ right;
    }
    difference == 0
}

#[derive(Debug)]
pub enum ServerError {
    InvalidOptions(String),
    Io(std::io::Error),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOptions(message) => write!(f, "invalid options: {message}"),
            Self::Io(error) => write!(f, "cannot bind: {error}"),
        }
    }
}

impl std::error::Error for ServerError {}

/// Builder for the server: settings first, then the parts tests replace (model factory, clock).
pub struct WebServer {
    options: WebOptions,
    llm: Option<LlmFactory>,
    skills: Option<Arc<dyn xiaoman_api::SkillRepository>>,
    clock: Option<Clock>,
    logger: Option<Logger>,
    transports: Option<TransportFactory>,
}

impl WebServer {
    pub fn new(options: WebOptions) -> Self {
        Self {
            options,
            llm: None,
            skills: None,
            clock: None,
            logger: None,
            transports: None,
        }
    }

    pub fn with_llm(mut self, llm: LlmFactory) -> Self {
        self.llm = Some(llm);
        self
    }

    pub fn with_skills(mut self, skills: Arc<dyn xiaoman_api::SkillRepository>) -> Self {
        self.skills = Some(skills);
        self
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    pub fn with_logger(mut self, logger: Logger) -> Self {
        self.logger = Some(logger);
        self
    }

    pub fn with_transports(mut self, transports: TransportFactory) -> Self {
        self.transports = Some(transports);
        self
    }

    pub fn state(&self) -> Result<Arc<AppState>, ServerError> {
        self.options
            .validate()
            .map_err(ServerError::InvalidOptions)?;
        let clock = self
            .clock
            .clone()
            .unwrap_or_else(|| Arc::new(xiaoman_api::now_millis));
        let llm = self
            .llm
            .clone()
            .unwrap_or_else(|| llm_factory(self.options.allow_private));
        let skills = self.skills.clone().unwrap_or_else(|| {
            Arc::new(ChatSkillRepository::new(self.options.skills_root.clone()))
                as Arc<dyn xiaoman_api::SkillRepository>
        });
        let limiter = StdioLimiter::new(self.options.max_stdio);
        let transports: TransportFactory = self.transports.clone().unwrap_or_else(|| {
            let limiter = limiter.clone();
            Arc::new(move |server: &xiaoman_mcp::McpServerConfig| limiter.transport(server))
        });
        let logger = self
            .logger
            .clone()
            .unwrap_or_else(|| Arc::new(|line: &str| println!("{line}")));
        let gateway = match self.options.class.clone() {
            Some(class) => Some(Arc::new(
                ClassGateway::new(
                    class,
                    self.options.timezone_offset_minutes,
                    clock.clone(),
                    logger.clone(),
                    self.options.allow_private,
                )
                .map_err(ServerError::InvalidOptions)?,
            )),
            None => None,
        };
        let claims = match (&gateway, self.options.claim.clone()) {
            (Some(gateway), Some(config)) => {
                let service = ClaimService::new(
                    config,
                    gateway.store(),
                    self.options.timezone_offset_minutes,
                    clock.clone(),
                    logger.clone(),
                    gateway.options().web_model_name().to_string(),
                )
                .map_err(ServerError::InvalidOptions)?;
                Some(Arc::new(service))
            }
            _ => None,
        };
        Ok(Arc::new(AppState {
            options: Arc::new(self.options.clone()),
            llm,
            skills,
            clock: clock.clone(),
            logger,
            catalog: McpCatalog::built_in(),
            transports,
            sessions: Mutex::new(SessionStore::new(
                self.options.max_sessions,
                self.options.session_millis,
                clock,
            )),
            static_files: StaticFiles::new(self.options.static_root_or_bundled()),
            gateway,
            claims,
        }))
    }

    pub async fn bind(self) -> Result<BoundServer, ServerError> {
        let state = self.state()?;
        let listener =
            tokio::net::TcpListener::bind((state.options.host.as_str(), state.options.port))
                .await
                .map_err(ServerError::Io)?;
        let port = listener.local_addr().map_err(ServerError::Io)?.port();
        let (shutdown, closed) = tokio::sync::oneshot::channel();
        let serving = state.clone();
        let task = tokio::spawn(async move {
            let router = router(serving);
            let _ = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = closed.await;
            })
            .await;
        });
        let reaper = {
            let state = state.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    if let Ok(mut sessions) = state.sessions.lock() {
                        sessions.expire();
                    }
                }
            })
        };
        // The Kotlin server writes usage back every few seconds instead of once per request.
        let flusher = state.gateway.as_ref().map(|gateway| {
            let gateway = gateway.clone();
            let interval = Duration::from_millis(gateway.options().flush_millis.max(100));
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    gateway.flush_quietly();
                }
            })
        });
        Ok(BoundServer {
            port,
            state,
            shutdown: Some(shutdown),
            task,
            reaper,
            flusher,
        })
    }
}

/// A running server; dropping it stops the listener and closes every session.
pub struct BoundServer {
    port: u16,
    state: Arc<AppState>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
    reaper: tokio::task::JoinHandle<()>,
    /// Writes class-token usage back while the server runs.
    flusher: Option<tokio::task::JoinHandle<()>>,
}

impl BoundServer {
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn state(&self) -> &Arc<AppState> {
        &self.state
    }

    pub async fn shutdown(mut self) {
        self.stop().await;
    }

    async fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.reaper.abort();
        if let Some(flusher) = self.flusher.take() {
            flusher.abort();
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut self.task).await;
        self.flush_gateway();
        self.state.sessions.lock().unwrap().close_all();
    }

    /// Pending class-token usage reaches the file even when the server stops right after a turn.
    fn flush_gateway(&self) {
        if let Some(gateway) = &self.state.gateway {
            gateway.flush_quietly();
        }
    }
}

impl Drop for BoundServer {
    fn drop(&mut self) {
        self.reaper.abort();
        if let Some(flusher) = self.flusher.take() {
            flusher.abort();
        }
        self.task.abort();
        self.flush_gateway();
        if let Ok(mut sessions) = self.state.sessions.lock() {
            sessions.close_all();
        }
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/info", get(info))
        .route("/api/catalog", get(catalog))
        .route("/api/session", post(new_session))
        .route("/api/chat", post(chat))
        .route("/api/answer", post(answer))
        .route("/api/mcp/test", post(mcp_test))
        .route("/api/claim/start", post(claim_start))
        .route("/api/claim/verify", post(claim_verify))
        // Class tokens authenticate `/v1` themselves: the Android app cannot send the access code.
        .route("/v1/{*rest}", any(v1))
        .route("/v1/", any(v1))
        .fallback(connect_or_static)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES + 1))
        .layer(middleware::from_fn_with_state(state.clone(), access_gate))
        .layer(middleware::from_fn_with_state(state.clone(), observe))
        .with_state(state)
}

/// `/v1/*`: the class gateway answers while the token file exists, reading the body itself so it can
/// use its own limit; until then the request falls through to the static files as before.
async fn v1(State(state): State<Arc<AppState>>, request: Request) -> Response {
    let gateway = state.gateway.clone().filter(|gateway| gateway.active());
    match gateway {
        Some(gateway) => gateway.handle_v1(request).await,
        None => static_or_not_found(State(state), request).await,
    }
}

/// The client address, when the server was bound with connect info; a router driven directly in a
/// test has none, and then only the forwarded header is trusted.
struct PeerAddr(Option<std::net::SocketAddr>);

impl<S> axum::extract::FromRequestParts<S> for PeerAddr
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let peer = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0);
        std::future::ready(Ok(Self(peer)))
    }
}

/// Behind Caddy (a loopback peer) the client is the last `X-Forwarded-For` entry, which Caddy sets itself.
fn client_ip(headers: &HeaderMap, peer: Option<std::net::SocketAddr>) -> String {
    let forwarded = || {
        headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.split(',').next_back())
            .map(str::trim)
            .filter(|entry| !entry.is_empty() && entry.len() <= 64)
            .map(str::to_string)
    };
    match peer {
        Some(peer) if peer.ip().is_loopback() => {
            forwarded().unwrap_or_else(|| peer.ip().to_string())
        }
        Some(peer) => peer.ip().to_string(),
        None => forwarded().unwrap_or_else(|| "direct".to_string()),
    }
}

async fn observe(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    // The class gateway writes its own `/v1` line, with the token label and the token count.
    let gateway_logged = path.starts_with("/v1/")
        && state
            .gateway
            .as_ref()
            .is_some_and(|gateway| gateway.active());
    if !gateway_logged {
        let route = if API_PATHS.contains(&path.as_str()) {
            path.as_str()
        } else if path.starts_with("/api/") {
            "/api/其他"
        } else {
            "/静态文件"
        };
        (state.logger)(&format!(
            "{method} {route} {} {}ms",
            response.status().as_u16(),
            started.elapsed().as_millis()
        ));
    }
    response
}

async fn access_gate(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    // The claim page is for students who have no access code yet; it is rate-limited instead.
    let open = path == "/api/info" || path.starts_with("/api/claim/");
    if path.starts_with("/api/") && !open && !state.authorized(request.headers()) {
        return Problem::unauthorized().into_response();
    }
    next.run(request).await
}

async fn method_not_allowed() -> Response {
    Problem::new(StatusCode::METHOD_NOT_ALLOWED, "请求方法不支持").into_response()
}

async fn info(State(state): State<Arc<AppState>>) -> Response {
    let claim = state
        .claims
        .as_ref()
        .map_or_else(|| json!({ "state": "off" }), |claims| claims.info());
    json_response(
        StatusCode::OK,
        &json!({
            "name": "Xiaoman",
            "version": "1.1",
            "accessCodeRequired": state.options.access_code_required(),
            "stdioMcp": state.options.stdio_mcp,
            "memory": "browser",
            "classTokens": state.gateway.as_ref().is_some_and(|gateway| gateway.active()),
            "claim": claim,
        }),
    )
}

async fn catalog(State(state): State<Arc<AppState>>) -> Response {
    let servers: Vec<Value> = state
        .catalog
        .servers
        .iter()
        .filter(|server| server.available_on(McpPlatform::Web) || !server.remote())
        .map(|server| {
            let mut described = McpCatalog::describe(server);
            if let Some(map) = described.as_object_mut() {
                map.insert(
                    "available".to_string(),
                    json!(server.remote() || state.options.stdio_mcp),
                );
                if !server.remote() {
                    map.insert(
                        "note".to_string(),
                        json!(if state.options.stdio_mcp {
                            "由服务器上的 Node.js 运行，第一次使用要多等一会儿"
                        } else {
                            "服务器没有开启这项服务（需要 Node.js），网页版暂不可用"
                        }),
                    );
                }
            }
            described
        })
        .collect();
    json_response(StatusCode::OK, &json!({ "servers": servers }))
}

async fn new_session(
    State(state): State<Arc<AppState>>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    if let Err(problem) = read_body(body) {
        return problem.into_response();
    }
    let session = state.sessions.lock().unwrap().new_session();
    json_response(StatusCode::OK, &json!({ "sessionId": session.id() }))
}

async fn answer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Some(session) = state.session(&headers) else {
        return Problem::not_found("会话已过期").into_response();
    };
    let value = match json_body(body) {
        Ok(value) => value,
        Err(problem) => return problem.into_response(),
    };
    if !session.channels().answer(&value) {
        return Problem::not_found("确认或问题已过期").into_response();
    }
    json_response(StatusCode::OK, &json!({ "ok": true }))
}

async fn mcp_test(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Some(session) = state.session(&headers) else {
        return Problem::not_found("会话已过期").into_response();
    };
    let value = match json_body(body) {
        Ok(value) => value,
        Err(problem) => return problem.into_response(),
    };
    session.remember_secrets(&value);
    let config = match build_mcp_config(&value, selection_options(&state), state.catalog).await {
        Ok(config) => config,
        Err(problem) => return problem.into_response(),
    };
    let provider = build_provider(
        config.clone(),
        session.channels().clone(),
        &state.transports,
        &state.clock,
    );
    let timeout = Duration::from_millis(state.options.chat_timeout_millis);
    let status = tokio::time::timeout(timeout, provider.status(true)).await;
    provider.close();
    let status = match status {
        Ok(status) => status,
        Err(_) => return Problem::internal().into_response(),
    };
    let servers: Vec<Value> = status
        .iter()
        .map(|item| {
            json!({
                "name": item.name,
                "title": item.title,
                "connected": item.connected,
                "toolCount": item.tool_count,
                "error": item.error.as_ref().map(|error| {
                    session.redact_text(error).chars().take(200).collect::<String>()
                }),
            })
        })
        .collect();
    let result = json!({ "servers": servers, "warnings": config.warnings });
    json_response(StatusCode::OK, &session.redact_json(&result))
}

async fn claim_start(
    State(state): State<Arc<AppState>>,
    PeerAddr(peer): PeerAddr,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    handle_claim(&state, peer, &headers, body, true).await
}

async fn claim_verify(
    State(state): State<Arc<AppState>>,
    PeerAddr(peer): PeerAddr,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    handle_claim(&state, peer, &headers, body, false).await
}

/// `/api/claim/{start,verify}`: the page of a student who has no access code yet.
async fn handle_claim(
    state: &AppState,
    peer: Option<std::net::SocketAddr>,
    headers: &HeaderMap,
    body: Result<Bytes, BytesRejection>,
    start: bool,
) -> Response {
    let Some(claims) = state.claims.as_ref() else {
        return Problem::not_found("自助领取没有开启").into_response();
    };
    let value = match json_body(body) {
        Ok(value) => value,
        Err(problem) => return problem.into_response(),
    };
    let ip = client_ip(headers, peer);
    let reply = if start {
        claims.start(&ip, &value).await
    } else {
        claims.verify(&ip, &value).await
    };
    claim_response(reply)
}

fn claim_response(reply: ClaimReply) -> Response {
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = json_response(status, &reply.body);
    let headers = response.headers_mut();
    if let Some(wait) = reply.retry_after {
        if let Ok(value) = axum::http::HeaderValue::from_str(&wait.to_string()) {
            headers.insert(header::RETRY_AFTER, value);
        }
    }
    headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

async fn connect_or_static(State(state): State<Arc<AppState>>, request: Request) -> Response {
    if state.options().local_connect && local_launch::is_connect_path(request.uri().path()) {
        // `into_make_service_with_connect_info` puts the peer in the extensions; tests that drive
        // the router directly have none, and the connect page then refuses like a remote request.
        let peer = request
            .extensions()
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|ConnectInfo(peer)| *peer);
        return local_launch::serve(&state, peer, request).await;
    }
    static_or_not_found(State(state), request).await
}

async fn static_or_not_found(State(state): State<Arc<AppState>>, request: Request) -> Response {
    if request.uri().path().starts_with("/api/") {
        return Problem::not_found("接口不存在").into_response();
    }
    state.static_files.serve(request).await
}

async fn chat(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let Some(session) = state.session(&headers) else {
        return Problem::not_found("会话已过期").into_response();
    };
    let value = match json_body(body) {
        Ok(value) => value,
        Err(problem) => return problem.into_response(),
    };
    if session.closed() {
        return Problem::not_found("会话已过期").into_response();
    }
    if !session.begin_turn() {
        return Problem::conflict("上一条消息还在处理").into_response();
    }
    session.remember_secrets(&value);
    if let Some(gateway) = &state.gateway {
        session.remember_secret(gateway.upstream_key());
    }
    let (sender, receiver) = mpsc::unbounded_channel::<Value>();
    session.channels().attach(sender);
    let task = tokio::spawn(run_turn(state.clone(), session.clone(), value));
    let abort = task.abort_handle();
    session.set_turn(abort.clone());
    let stream = TurnStream {
        receiver,
        abort,
        session,
    };
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new().interval(Duration::from_millis(state.options.heartbeat_millis)),
        )
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    response
}

/// The events of one turn, redacted as they pass; dropping it (client gone) stops the turn.
struct TurnStream {
    receiver: mpsc::UnboundedReceiver<Value>,
    abort: AbortHandle,
    session: Arc<WebSession>,
}

impl Stream for TurnStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.receiver.poll_recv(context) {
            Poll::Ready(Some(value)) => {
                let value = truncate_approval(this.session.redact_json(&value));
                Poll::Ready(Some(Ok(Event::default().data(value.to_string()))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for TurnStream {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

fn truncate_approval(mut value: Value) -> Value {
    if value.get("type").and_then(Value::as_str) != Some("approval") {
        return value;
    }
    if let Some(map) = value.as_object_mut() {
        if let Some(arguments) = map.get("arguments").and_then(Value::as_str) {
            let clipped: String = arguments.chars().take(500).collect();
            map.insert("arguments".to_string(), Value::String(clipped));
        }
    }
    value
}

enum TurnError {
    /// Shown to the page as is; model configuration problems.
    Message(String),
    /// Unexpected failures keep the generic message so nothing internal leaks.
    Internal,
}

/// Ends the turn even when the task is aborted by a disconnect.
struct TurnGuard {
    session: Arc<WebSession>,
    clock: Clock,
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.session.finish_turn((self.clock)());
    }
}

async fn run_turn(state: Arc<AppState>, session: Arc<WebSession>, body: Value) {
    let _guard = TurnGuard {
        session: session.clone(),
        clock: state.clock.clone(),
    };
    let outcome = tokio::time::timeout(
        Duration::from_millis(state.options.chat_timeout_millis),
        turn(&state, session.clone(), body),
    )
    .await;
    match outcome {
        Ok(Ok(())) => session.channels().emit(json!({ "type": "done" })),
        Ok(Err(TurnError::Message(message))) => {
            session
                .channels()
                .emit(json!({ "type": "error", "message": message }));
            session.channels().emit(json!({ "type": "done" }));
        }
        Ok(Err(TurnError::Internal)) => {
            session
                .channels()
                .emit(json!({ "type": "error", "message": "处理失败，请检查配置后重试" }));
            session.channels().emit(json!({ "type": "done" }));
        }
        Err(_) => {
            session
                .channels()
                .emit(json!({ "type": "error", "message": "本轮处理超时，请重试" }));
            session.channels().emit(json!({ "type": "done" }));
        }
    }
}

async fn turn(state: &AppState, session: Arc<WebSession>, body: Value) -> Result<(), TurnError> {
    let model = body.get("model").and_then(Value::as_object);
    let class_token = text(model, "classToken").trim().to_string();
    // A class token replaces the model address and key with the instructor's; a denial stops the turn.
    let class_factory = if class_token.is_empty() {
        None
    } else {
        let Some(gateway) = state.gateway.as_ref() else {
            return Err(TurnError::Message(
                "服务器没有开启课堂令牌，请改用自己的 API Key".to_string(),
            ));
        };
        if let Some(message) = gateway.denial(&class_token) {
            return Err(TurnError::Message(message));
        }
        Some(gateway.factory(&class_token))
    };
    if class_factory.is_none() {
        let base_url = text(model, "baseUrl").trim().to_string();
        let model_name = text(model, "model").trim().to_string();
        let api_key = text(model, "apiKey").trim().to_string();
        if base_url.is_empty() || model_name.is_empty() || api_key.is_empty() {
            return Err(TurnError::Message(
                "请先配置模型地址、模型名称和 API Key".to_string(),
            ));
        }
        if !valid_model_url(&base_url) {
            return Err(TurnError::Message("模型地址无效".to_string()));
        }
        if !state.options.allow_private && !is_public_url(&base_url).await {
            return Err(TurnError::Message("模型地址不允许指向内网".to_string()));
        }
    }
    let config = build_mcp_config(&body, selection_options(state), state.catalog)
        .await
        .map_err(|_| TurnError::Internal)?;
    for warning in &config.warnings {
        session
            .channels()
            .emit(json!({ "type": "warning", "message": warning }));
    }
    if let Some(loaded) = BrowserMemory::parse(&body) {
        session.memory().replace_all(loaded.notes);
        if loaded.skipped > 0 {
            session.channels().emit(json!({
                "type": "warning",
                "message": format!("有 {} 条本地记忆格式不对或超出上限，这次没有带上", loaded.skipped),
            }));
        }
    }
    let factory = class_factory.unwrap_or_else(|| state.llm.clone());
    let agent = session
        .assemble(AssembleInputs {
            body: &body,
            config: &config,
            llm: &factory,
            transports: &state.transports,
            skills: state.skills.clone(),
            offset_minutes: state.options.timezone_offset_minutes,
            clock: state.clock.clone(),
        })
        .map_err(|_| TurnError::Internal)?;
    let listener: Arc<dyn AgentListener> = Arc::new(TurnEmitter {
        session: session.clone(),
    });
    let stop = Arc::new(SessionStop::new(session.clone()));
    agent
        .handle(&text(body.as_object(), "message"), listener, stop)
        .await;
    Ok(())
}

fn selection_options(state: &AppState) -> SelectionOptions {
    SelectionOptions {
        allow_private: state.options.allow_private,
        stdio_mcp: state.options.stdio_mcp,
    }
}

fn valid_model_url(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(parsed) => matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
        Err(_) => false,
    }
}

/// Streams the events of one turn to the page.
struct TurnEmitter {
    session: Arc<WebSession>,
}

impl AgentListener for TurnEmitter {
    fn on_event(&self, event: &AgentEvent) {
        let value = match event {
            AgentEvent::Reply { text, .. } => json!({ "type": "reply", "text": text }),
            AgentEvent::ToolCalled {
                tool, ok, summary, ..
            } => {
                json!({ "type": "tool", "tool": tool, "ok": ok, "summary": summary })
            }
            AgentEvent::MemoryUpdated { permalink, .. } => {
                match self.session.memory().snapshot(permalink) {
                    Some(note) => BrowserMemory::saved(&note),
                    None => BrowserMemory::deleted(permalink),
                }
            }
            AgentEvent::Failed { message, .. } => json!({ "type": "error", "message": message }),
            _ => return,
        };
        self.session.channels().emit(value);
    }
}

fn read_body(body: Result<Bytes, BytesRejection>) -> Result<Bytes, Problem> {
    match body {
        Ok(bytes) if bytes.len() <= MAX_BODY_BYTES => Ok(bytes),
        _ => Err(Problem::too_large()),
    }
}

fn json_body(body: Result<Bytes, BytesRejection>) -> Result<Value, Problem> {
    let bytes = read_body(body)?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value @ Value::Object(_)) => Ok(value),
        _ => Err(Problem::bad_request("请求内容需要是合法 JSON 对象")),
    }
}

fn text(object: Option<&Map<String, Value>>, key: &str) -> String {
    object
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl WebServer {
    /// The router this server would serve; useful when another process owns the socket.
    pub fn router(&self) -> Result<Router, ServerError> {
        Ok(router(self.state()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registered_routes_answer_without_a_socket() {
        let app = WebServer::new(WebOptions::default()).router().unwrap();
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/session")
            .header("content-type", "application/json")
            .body(axum::body::Body::from("{}"))
            .unwrap();
        let response = tower::ServiceExt::oneshot(app, request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn access_codes_compare_in_constant_time() {
        assert!(constant_time_eq(b"access", b"access"));
        assert!(!constant_time_eq(b"access", b"acc"));
        assert!(!constant_time_eq(b"access", b"accesz"));
    }
}
