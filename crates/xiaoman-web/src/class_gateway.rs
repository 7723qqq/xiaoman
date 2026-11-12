//! Class-token mode: the settings, the metered model client the page uses in that mode, and the
//! `/v1` OpenAI-compatible passthrough that lets an OpenAI client (the Android app) talk to the
//! instructor's upstream.
//!
//! The page sends a class token instead of a model address and key; the server then calls the
//! instructor's upstream with the instructor's key, counts the model tokens of every call and
//! records them against the token. Both entry points share one limiter and one token store, so a
//! token's usage is the same whether it arrives through `/api/chat` or `/v1`.

use std::convert::Infallible;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::{Body, Bytes};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::Stream;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use xiaoman_api::{ChatRequest, ChatResponse, ContentPart, LlmClient, LlmError, ModelConfig};
use xiaoman_llm::{ClientOptions, OpenAiCompatibleClient};

use crate::class_tokens::{ClassDenied, ClassToken, ClassTokenStore};
use crate::class_v1::{
    BodyRead, ClassLimiter, IMAGE_TOKENS, Slot, bearer, content_length, drain_body, error_response,
    estimate_completion, estimate_prompt, read_body, redact, upstream_error, usage_of,
};
use crate::memory::Clock;
use crate::server::Logger;
use crate::session::LlmFactory;
use crate::tokens_admin::DEFAULT_FILE;

/// Volcano Ark, the upstream the Kotlin server defaults to.
pub const DEFAULT_UPSTREAM: &str = "https://ark.cn-beijing.volces.com/api/v3";

/// Class-token mode settings. The feature is on only when an upstream key and a model whitelist are
/// configured and the token file exists. `models` maps the name students type to the upstream model id.
#[derive(Clone, PartialEq, Eq)]
pub struct ClassOptions {
    pub upstream_key: String,
    /// Ordered: without `web_model` the first name is the one the page mentions.
    pub models: Vec<(String, String)>,
    pub upstream_url: String,
    pub tokens_file: PathBuf,
    /// Whitelist name used by the page's class-token mode; defaults to the first one.
    pub web_model: Option<String>,
    pub timeout_seconds: i64,
    pub flush_millis: u64,
    /// How often the server notices a token file replaced by the admin command.
    pub reload_millis: i64,
    /// Concurrency and requests per minute for one token, and the global concurrency cap.
    pub per_token_concurrency: u32,
    pub per_token_per_minute: u32,
    pub max_concurrent: u32,
    /// Request body limit of the `/v1` routes, in bytes.
    pub max_body_bytes: u64,
    /// Ark `thinking.type` added to `/v1` requests that do not set one (deep thinking bills as
    /// output); `None` forwards the request unchanged.
    pub thinking: Option<String>,
}

/// Settings whose `Debug` keeps the instructor's key out of logs and traces.
impl std::fmt::Debug for ClassOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClassOptions")
            .field("upstream_url", &self.upstream_url)
            .field(
                "models",
                &self.models.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            )
            .field("tokens_file", &self.tokens_file)
            .field("web_model", &self.web_model)
            .field("timeout_seconds", &self.timeout_seconds)
            .field("flush_millis", &self.flush_millis)
            .field("reload_millis", &self.reload_millis)
            .field("per_token_concurrency", &self.per_token_concurrency)
            .field("per_token_per_minute", &self.per_token_per_minute)
            .field("max_concurrent", &self.max_concurrent)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("thinking", &self.thinking)
            .field("upstream_key", &"***")
            .finish()
    }
}

impl ClassOptions {
    pub fn new(
        upstream_key: impl Into<String>,
        models: Vec<(String, String)>,
        upstream_url: impl Into<String>,
        tokens_file: PathBuf,
    ) -> Self {
        Self {
            upstream_key: upstream_key.into(),
            models,
            upstream_url: upstream_url.into(),
            tokens_file,
            web_model: None,
            timeout_seconds: 180,
            flush_millis: 5_000,
            reload_millis: 1_000,
            per_token_concurrency: 2,
            per_token_per_minute: 30,
            max_concurrent: 32,
            max_body_bytes: 8 * 1024 * 1024,
            thinking: Some("disabled".to_string()),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.upstream_key.trim().is_empty() || self.models.is_empty() {
            return Err("课堂令牌需要上游 Key 和模型白名单".to_string());
        }
        if self.timeout_seconds <= 0 || self.flush_millis == 0 {
            return Err("课堂令牌的超时或写回间隔无效".to_string());
        }
        if self.per_token_concurrency == 0
            || self.per_token_per_minute == 0
            || self.max_concurrent == 0
            || self.max_body_bytes == 0
        {
            return Err("课堂令牌的并发、频次或请求体上限无效".to_string());
        }
        if let Some(mode) = &self.thinking {
            if !matches!(mode.as_str(), "disabled" | "enabled" | "auto") {
                return Err("XIAOMAN_CLASS_THINKING 只能是 disabled/enabled/auto/keep".to_string());
            }
        }
        if !self.upstream_url.starts_with("https://") && !self.upstream_url.starts_with("http://") {
            return Err("XIAOMAN_CLASS_UPSTREAM_URL 无效".to_string());
        }
        if let Some(web) = &self.web_model {
            if !self.models.iter().any(|(name, _)| name == web) {
                return Err("XIAOMAN_CLASS_WEB_MODEL 不在白名单里".to_string());
            }
        }
        Ok(())
    }

    /// The whitelist name the page mentions; `validate` guarantees it exists.
    pub fn web_model_name(&self) -> &str {
        self.web_model
            .as_deref()
            .filter(|name| self.models.iter().any(|(key, _)| key == name))
            .or_else(|| self.models.first().map(|(name, _)| name.as_str()))
            .unwrap_or_default()
    }

    pub fn real_model(&self, name: &str) -> Option<&str> {
        self.models
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, real)| real.as_str())
    }

    /// Reads `XIAOMAN_CLASS_*`. `None` (off) with the reason when something required is missing.
    pub fn from_env(
        env: &dyn Fn(&str) -> Option<String>,
        tokens_file: Option<PathBuf>,
    ) -> Result<(Option<ClassOptions>, String), String> {
        let value = |name: &str| {
            env(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let key = match value("XIAOMAN_CLASS_UPSTREAM_KEY") {
            Some(key) => Some(key),
            None => match value("XIAOMAN_CLASS_UPSTREAM_KEY_FILE") {
                Some(file) => match std::fs::read_to_string(&file) {
                    Ok(key) => Some(key.trim().to_string()),
                    Err(_) => return Ok((None, format!("读不到上游 Key 文件 {file}"))),
                },
                None => None,
            },
        };
        let Some(key) = key.filter(|key| !key.is_empty()) else {
            return Ok((
                None,
                "未配置上游 Key（XIAOMAN_CLASS_UPSTREAM_KEY_FILE）".to_string(),
            ));
        };
        let Some(models) = value("XIAOMAN_CLASS_MODELS") else {
            return Ok((None, "未配置模型白名单（XIAOMAN_CLASS_MODELS）".to_string()));
        };
        let models = parse_models(&models)?;
        if models.is_empty() {
            return Ok((None, "模型白名单为空".to_string()));
        }
        let upstream_url =
            value("XIAOMAN_CLASS_UPSTREAM_URL").unwrap_or_else(|| DEFAULT_UPSTREAM.to_string());
        let upstream_url = upstream_url.trim_end_matches('/').to_string();
        let web_model = value("XIAOMAN_CLASS_WEB_MODEL");
        if let Some(web) = &web_model {
            if !models.iter().any(|(name, _)| name == web) {
                return Err("XIAOMAN_CLASS_WEB_MODEL 不在白名单里".to_string());
            }
        }
        // `keep` forwards whatever the client sent; the default turns deep thinking off so only the
        // answer, not the reasoning, is billed as output.
        let thinking_text =
            value("XIAOMAN_CLASS_THINKING").unwrap_or_else(|| "disabled".to_string());
        let thinking = match thinking_text.as_str() {
            "keep" => None,
            mode @ ("disabled" | "enabled" | "auto") => Some(mode.to_string()),
            _ => return Err("XIAOMAN_CLASS_THINKING 只能是 disabled/enabled/auto/keep".to_string()),
        };
        let tokens_file = tokens_file
            .or_else(|| value("XIAOMAN_CLASS_TOKENS").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_FILE));
        let options = ClassOptions {
            web_model,
            thinking,
            ..ClassOptions::new(key, models, upstream_url, tokens_file)
        };
        let options = ClassOptions {
            per_token_concurrency: number("XIAOMAN_CLASS_TOKEN_CONCURRENCY", 2, &value)?,
            per_token_per_minute: number("XIAOMAN_CLASS_TOKEN_RPM", 30, &value)?,
            max_concurrent: number("XIAOMAN_CLASS_MAX_CONCURRENT", 32, &value)?,
            max_body_bytes: u64::from(number("XIAOMAN_CLASS_MAX_BODY_MB", 8, &value)?)
                * 1024
                * 1024,
            ..options
        };
        options.validate()?;
        Ok((Some(options), "已开启".to_string()))
    }
}

/// One positive integer setting; a missing variable takes the default, a bad one is an error.
fn number(name: &str, default: u32, value: &dyn Fn(&str) -> Option<String>) -> Result<u32, String> {
    match value(name) {
        Some(text) => text
            .parse::<u32>()
            .ok()
            .filter(|number| *number > 0)
            .ok_or_else(|| format!("{name} 需要正整数")),
        None => Ok(default),
    }
}

/// `alias=real-id` or `real-id`, separated by commas, semicolons or whitespace.
pub fn parse_models(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut models: Vec<(String, String)> = Vec::new();
    for entry in text
        .split([',', ';', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let (name, real) = match entry.split_once('=') {
            Some((name, real)) => (name.trim(), real.trim()),
            None => (entry, entry),
        };
        if name.is_empty() || real.is_empty() || !safe_model(name) || !safe_model(real) {
            return Err("模型白名单格式无效".to_string());
        }
        match models.iter_mut().find(|(key, _)| key == name) {
            Some(entry) => entry.1 = real.to_string(),
            None => models.push((name.to_string(), real.to_string())),
        }
    }
    Ok(models)
}

fn safe_model(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | ':' | '/' | '-')
        })
}

/// The token store, the limiter and the instructor upstream shared by `/api/chat` and `/v1`.
pub struct ClassGateway {
    options: ClassOptions,
    store: Arc<ClassTokenStore>,
    http: reqwest::Client,
    /// `/v1` forwarding: one timeout per read instead of a total, like the Kotlin OkHttp client, so
    /// a long stream is not cut off while a stalled one still is.
    v1_http: reqwest::Client,
    limiter: Arc<ClassLimiter>,
    logger: Logger,
    chat_url: String,
}

impl ClassGateway {
    pub fn new(
        options: ClassOptions,
        offset_minutes: i32,
        clock: Clock,
        logger: Logger,
        allow_private: bool,
    ) -> Result<Self, String> {
        options.validate()?;
        let resolver = || Arc::new(crate::safe_dns::SafeDns { allow_private });
        let http = reqwest::Client::builder()
            .dns_resolver(resolver())
            .timeout(Duration::from_secs(options.timeout_seconds.max(1) as u64))
            .build()
            .map_err(|error| format!("模型客户端初始化失败（{error}）"))?;
        let v1_http = reqwest::Client::builder()
            .dns_resolver(resolver())
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(options.timeout_seconds.max(1) as u64))
            .build()
            .map_err(|error| format!("模型客户端初始化失败（{error}）"))?;
        let store = Arc::new(ClassTokenStore::new(
            options.tokens_file.clone(),
            offset_minutes,
            clock.clone(),
            options.reload_millis,
        ));
        let limiter = Arc::new(ClassLimiter::new(
            options.per_token_concurrency,
            options.per_token_per_minute,
            options.max_concurrent,
            clock,
        ));
        let chat_url = format!(
            "{}/chat/completions",
            options.upstream_url.trim_end_matches('/')
        );
        Ok(Self {
            options,
            store,
            http,
            v1_http,
            limiter,
            logger,
            chat_url,
        })
    }

    pub fn options(&self) -> &ClassOptions {
        &self.options
    }

    pub fn store(&self) -> Arc<ClassTokenStore> {
        self.store.clone()
    }

    pub fn upstream_key(&self) -> &str {
        &self.options.upstream_key
    }

    /// True once the token file exists; until then the page's class-token mode stays off.
    pub fn active(&self) -> bool {
        self.store.exists()
    }

    /// Why this class token cannot be used right now, exactly as the page shows it.
    pub fn denial(&self, plain: &str) -> Option<String> {
        if !self.active() {
            return Some("服务器没有开启课堂令牌，请改用自己的 API Key".to_string());
        }
        match self.store.find(plain) {
            None => Some("课堂令牌无效".to_string()),
            Some(token) => self.store.check(&token).map(|denied| denied.message),
        }
    }

    /// An admitted request: the token and the limiter place it holds until dropped.
    pub fn admit(&self, plain: Option<&str>) -> Result<(ClassToken, Slot), ClassDenied> {
        let Some(plain) = plain.map(str::trim).filter(|plain| !plain.is_empty()) else {
            return Err(ClassDenied::new(
                401,
                "missing_token",
                "缺少课堂令牌（Authorization: Bearer <令牌>）",
                None,
            ));
        };
        let Some(token) = self.store.find(plain) else {
            return Err(ClassDenied::new(401, "invalid_token", "课堂令牌无效", None));
        };
        if let Some(denied) = self.store.check(&token) {
            return Err(denied);
        }
        let slot = self.limiter.acquire(&token.hash)?;
        Ok((token, slot))
    }

    /// A client factory for one turn: every model call is checked against the token, limited and
    /// metered, exactly like a `/v1` request.
    pub fn factory(&self, plain: &str) -> LlmFactory {
        let options = self.options.clone();
        let store = self.store.clone();
        let limiter = self.limiter.clone();
        let http = self.http.clone();
        let logger = self.logger.clone();
        let plain = plain.to_string();
        Arc::new(move |requested: &ModelConfig| {
            let real = options
                .real_model(options.web_model_name())
                .unwrap_or_default();
            let config = ModelConfig {
                role: requested.role,
                base_url: options.upstream_url.clone(),
                model: real.to_string(),
                api_key: options.upstream_key.clone(),
                temperature: requested.temperature,
                max_tokens: requested.max_tokens,
                timeout_seconds: options.timeout_seconds,
                extra_headers: Default::default(),
            };
            let inner = OpenAiCompatibleClient::with_options(
                config.clone(),
                ClientOptions {
                    max_retries: 1,
                    http: Some(http.clone()),
                    ..Default::default()
                },
            );
            Arc::new(MeteredClient {
                config,
                inner,
                store: store.clone(),
                limiter: limiter.clone(),
                plain: plain.clone(),
                logger: logger.clone(),
            }) as Arc<dyn LlmClient>
        })
    }

    /// Writes pending usage; a failure is logged and retried on the next flush.
    pub fn flush_quietly(&self) {
        if let Err(error) = self.store.flush() {
            (self.logger)(&format!("课堂令牌用量写入失败（{error}），稍后重试"));
        }
    }

    // ---- /v1 OpenAI-compatible forwarding ----

    /// `/v1/models` and `/v1/chat/completions` for a class token. The router calls this only while
    /// the token file exists, so a token created during a run turns the feature on.
    pub async fn handle_v1(&self, request: axum::extract::Request) -> Response {
        let log = RequestLog {
            method: request.method().clone(),
            route: v1_route(request.uri().path()),
            started: Instant::now(),
            logger: self.logger.clone(),
        };
        let mut report = Report::default();
        let response = self.serve_v1(request, &mut report, &log).await;
        if !report.deferred {
            log.emit(response.status().as_u16(), &report.tag, &report.note);
        }
        response
    }

    async fn serve_v1(
        &self,
        request: axum::extract::Request,
        report: &mut Report,
        log: &RequestLog,
    ) -> Response {
        match request.uri().path() {
            "/v1/models" => {
                if request.method() != Method::GET {
                    return error_response(&method_not_allowed());
                }
                let (token, _slot) = match self.admit(bearer(request.headers())) {
                    Ok(admitted) => admitted,
                    Err(denied) => {
                        report.note = denied.code.clone();
                        return error_response(&denied);
                    }
                };
                report.tag = token.tag();
                models_response(&self.options.models)
            }
            "/v1/chat/completions" => self.chat_v1(request, report, log).await,
            _ => error_response(&ClassDenied::new(404, "not_found", "接口不存在", None)),
        }
    }

    /// Forwards one chat completion; the log note is the token count, never the content.
    async fn chat_v1(
        &self,
        request: axum::extract::Request,
        report: &mut Report,
        log: &RequestLog,
    ) -> Response {
        if request.method() != Method::POST {
            return error_response(&method_not_allowed());
        }
        let max_body = self.options.max_body_bytes.max(1);
        let plain = bearer(request.headers()).map(str::to_string);
        let (token, slot) = match self.admit(plain.as_deref()) {
            Ok(admitted) => admitted,
            Err(denied) => {
                report.note = denied.code.clone();
                // The rest of a rejected body is read and dropped, so the client sees the error.
                drain_body(request.into_body(), max_body).await;
                return error_response(&denied);
            }
        };
        report.tag = token.tag();
        let hash = token.hash.clone();
        let declared = content_length(request.headers());
        let body = match read_body(request.into_body(), declared, max_body).await {
            BodyRead::Body(body) => body,
            BodyRead::TooLarge => {
                report.note = "request_too_large".to_string();
                return error_response(&self.too_large());
            }
            BodyRead::Failed => {
                report.note = "server_error".to_string();
                return error_response(&server_error());
            }
        };
        let root: Value = match serde_json::from_slice::<Value>(&body) {
            Ok(value) if value.is_object() => value,
            _ => {
                report.note = "bad_json".to_string();
                return error_response(&ClassDenied::new(
                    400,
                    "invalid_request_error",
                    "请求内容需要是合法 JSON 对象",
                    None,
                ));
            }
        };
        let name = root
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Some(real) = self.options.real_model(&name) else {
            report.note = "model_not_allowed".to_string();
            let names: Vec<&str> = self
                .options
                .models
                .iter()
                .map(|(name, _)| name.as_str())
                .collect();
            let shown: String = name.chars().take(64).collect();
            return error_response(&ClassDenied::new(
                400,
                "model_not_allowed",
                format!("模型「{shown}」不可用，可用：{}", names.join("、")),
                None,
            ));
        };
        let stream = root.get("stream").and_then(Value::as_bool).unwrap_or(false);
        let client_usage = stream
            && root
                .get("stream_options")
                .and_then(Value::as_object)
                .and_then(|options| options.get("include_usage"))
                .and_then(Value::as_bool)
                == Some(true);
        let mut outgoing = outgoing_body(&root, real, stream);
        if let (Some(mode), Value::Object(map)) = (&self.options.thinking, &mut outgoing) {
            map.entry("thinking")
                .or_insert_with(|| json!({ "type": mode }));
        }
        let prompt_estimate = estimate_prompt(&root);
        let deadline = Duration::from_secs(self.options.timeout_seconds.max(1) as u64);
        let request = self
            .v1_http
            .post(&self.chat_url)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", self.options.upstream_key),
            )
            .header(
                header::ACCEPT,
                if stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
            .body(outgoing.to_string());
        if !stream {
            let sent = tokio::time::timeout(deadline, async move {
                let response = request.send().await?;
                let status = response.status();
                let text = response.text().await?;
                Ok::<(reqwest::StatusCode, String), reqwest::Error>((status, text))
            })
            .await;
            let (status, text) = match sent {
                Ok(Ok(pair)) => pair,
                Ok(Err(error)) => {
                    self.store.record(&hash, 0);
                    report.note = "upstream_unavailable".to_string();
                    return error_response(&upstream_failure(&error));
                }
                Err(_) => {
                    self.store.record(&hash, 0);
                    report.note = "upstream_timeout".to_string();
                    return error_response(&upstream_timeout());
                }
            };
            if !status.is_success() {
                self.store.record(&hash, 0);
                report.note = format!("upstream_{}", status.as_u16());
                return error_response(&upstream_error(
                    status.as_u16(),
                    &text,
                    &self.options.upstream_key,
                ));
            }
            let parsed: Option<Value> = serde_json::from_str(&text).ok();
            let used = parsed.as_ref().and_then(usage_of);
            let counted =
                used.unwrap_or_else(|| prompt_estimate + estimate_completion(parsed.as_ref()));
            self.store.record(&hash, counted);
            report.note = format!(
                "tokens={counted}{}",
                if used.is_none() { "(估算)" } else { "" }
            );
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                redact(&text, &self.options.upstream_key),
            )
                .into_response();
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                self.store.record(&hash, 0);
                report.note = "upstream_unavailable".to_string();
                return error_response(&upstream_failure(&error));
            }
        };
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            self.store.record(&hash, 0);
            report.note = format!("upstream_{status}");
            return error_response(&upstream_error(status, &body, &self.options.upstream_key));
        }
        let stream = ForwardedStream {
            upstream: Some(response),
            pending: None,
            buffer: Vec::new(),
            client_usage,
            skip_blank: false,
            key: self.options.upstream_key.clone(),
            account: StreamAccount {
                store: self.store.clone(),
                hash,
                prompt_estimate,
                used: None,
                produced: 0,
                slot,
                tag: report.tag.clone(),
                log: log.clone(),
                finished: false,
            },
        };
        // The stream reports its own token count once the upstream body is done.
        report.deferred = true;
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from_stream(stream))
            .unwrap_or_else(|_| error_response(&server_error()))
    }

    fn too_large(&self) -> ClassDenied {
        let megabytes = self.options.max_body_bytes / 1024 / 1024;
        ClassDenied::new(
            413,
            "request_too_large",
            format!("请求内容超过 {megabytes}MB（截图太大或对话太长）"),
            None,
        )
    }
}

/// Where one `/v1` request's log line goes, and when.
#[derive(Clone)]
struct RequestLog {
    method: Method,
    route: &'static str,
    started: Instant,
    logger: Logger,
}

impl RequestLog {
    fn emit(&self, status: u16, tag: &str, note: &str) {
        (self.logger)(
            format!(
                "{} {} {status} {}ms {tag} {note}",
                self.method,
                self.route,
                self.started.elapsed().as_millis()
            )
            .trim_end(),
        );
    }
}

/// What the log line will say; a forwarded stream finishes it after the body is done.
struct Report {
    tag: String,
    note: String,
    deferred: bool,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            tag: "-".to_string(),
            note: String::new(),
            deferred: false,
        }
    }
}

/// The route name the log may carry; anything else under `/v1` is not echoed.
fn v1_route(path: &str) -> &'static str {
    match path {
        "/v1/chat/completions" => "/v1/chat/completions",
        "/v1/models" => "/v1/models",
        _ => "/v1/其他",
    }
}

/// The whitelist as the clients read it; the real upstream IDs stay on the server.
fn models_response(models: &[(String, String)]) -> Response {
    let data: Vec<Value> = models
        .iter()
        .map(|(name, _)| {
            json!({ "id": name, "object": "model", "created": 0, "owned_by": "xiaoman-class" })
        })
        .collect();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        json!({ "object": "list", "data": data }).to_string(),
    )
        .into_response()
}

/// The request as the upstream receives it: the alias replaced by the real ID, usage asked for on
/// every stream.
fn outgoing_body(root: &Value, real: &str, stream: bool) -> Value {
    let mut body = root.as_object().cloned().unwrap_or_default();
    body.insert("model".to_string(), json!(real));
    if stream {
        let mut options = body
            .get("stream_options")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        options.insert("include_usage".to_string(), json!(true));
        body.insert("stream_options".to_string(), Value::Object(options));
    }
    Value::Object(body)
}

fn method_not_allowed() -> ClassDenied {
    ClassDenied::new(405, "method_not_allowed", "请求方法不支持", None)
}

fn server_error() -> ClassDenied {
    ClassDenied::new(500, "server_error", "请求处理失败", None)
}

fn upstream_timeout() -> ClassDenied {
    ClassDenied::new(504, "upstream_timeout", "模型服务响应超时，请重试", None)
}

/// A timeout is a 504; any other transport failure a 502.
fn upstream_failure(error: &reqwest::Error) -> ClassDenied {
    if error.is_timeout() {
        upstream_timeout()
    } else {
        ClassDenied::new(
            502,
            "upstream_unavailable",
            "连接模型服务失败，请稍后重试",
            None,
        )
    }
}

/// One upstream SSE body, forwarded line by line. Dropping it (the client went away) stops the
/// upstream call and still records what the stream had produced.
struct ForwardedStream {
    upstream: Option<reqwest::Response>,
    pending: Option<BoxFuture<'static, (reqwest::Response, reqwest::Result<Option<Bytes>>)>>,
    buffer: Vec<u8>,
    client_usage: bool,
    skip_blank: bool,
    key: String,
    account: StreamAccount,
}

impl ForwardedStream {
    /// One upstream line; `Some(bytes)` is what the client receives.
    fn handle_line(&mut self, line: &str) -> Option<Bytes> {
        if line.is_empty() && self.skip_blank {
            self.skip_blank = false;
            return None;
        }
        self.skip_blank = false;
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            let chunk = if data == "[DONE]" {
                None
            } else {
                serde_json::from_str::<Value>(data)
                    .ok()
                    .filter(Value::is_object)
            };
            if let Some(chunk) = &chunk {
                if let Some(used) = usage_of(chunk) {
                    self.account.used = Some(used);
                }
                self.account.produced += estimate_completion(Some(chunk));
                // The usage-only final chunk goes only to clients that asked for it.
                let usage_only = chunk.get("usage").is_some_and(Value::is_object)
                    && chunk
                        .get("choices")
                        .and_then(Value::as_array)
                        .is_none_or(Vec::is_empty);
                if !self.client_usage && usage_only {
                    self.skip_blank = true;
                    return None;
                }
            }
        }
        Some(Bytes::from(format!("{}\n", redact(line, &self.key))))
    }
}

impl Stream for ForwardedStream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            // 1. A complete line, if the buffer already holds one.
            if let Some(position) = this.buffer.iter().position(|byte| *byte == b'\n') {
                let rest = this.buffer.split_off(position + 1);
                let mut line = std::mem::replace(&mut this.buffer, rest);
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                let text = String::from_utf8_lossy(&line).to_string();
                if let Some(bytes) = this.handle_line(&text) {
                    return Poll::Ready(Some(Ok(bytes)));
                }
                continue;
            }
            // 2. The read that is already in flight; it must be polled before another starts.
            if let Some(pending) = this.pending.as_mut() {
                match pending.as_mut().poll(context) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready((upstream, chunk)) => {
                        this.pending = None;
                        match chunk {
                            Ok(Some(bytes)) => {
                                this.upstream = Some(upstream);
                                this.buffer.extend_from_slice(&bytes);
                            }
                            // The end of the body, or an upstream that broke off: forward what we
                            // have and finish.
                            Ok(None) | Err(_) => this.upstream = None,
                        }
                        continue;
                    }
                }
            }
            // 3. Start the next read, or finish when the upstream body has ended.
            match this.upstream.take() {
                Some(upstream) => {
                    this.pending = Some(Box::pin(async move {
                        let mut upstream = upstream;
                        let chunk = upstream.chunk().await;
                        (upstream, chunk)
                    }));
                }
                None => {
                    // A last line without a trailing newline is still a line.
                    if !this.buffer.is_empty() {
                        let line = std::mem::take(&mut this.buffer);
                        let text = String::from_utf8_lossy(&line).to_string();
                        if let Some(bytes) = this.handle_line(&text) {
                            this.account.finish(false);
                            return Poll::Ready(Some(Ok(bytes)));
                        }
                    }
                    this.account.finish(false);
                    return Poll::Ready(None);
                }
            }
        }
    }
}

/// Records what one forwarded stream cost, once, however the stream ends.
struct StreamAccount {
    store: Arc<ClassTokenStore>,
    hash: String,
    prompt_estimate: i64,
    used: Option<i64>,
    produced: i64,
    slot: Slot,
    tag: String,
    log: RequestLog,
    finished: bool,
}

impl StreamAccount {
    fn finish(&mut self, interrupted: bool) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.slot.release();
        let counted = self.used.unwrap_or(self.prompt_estimate + self.produced);
        self.store.record(&self.hash, counted);
        let mut note = format!(
            "stream tokens={counted}{}",
            if self.used.is_none() { "(估算)" } else { "" }
        );
        if interrupted {
            note.push_str(" 中断");
        }
        self.log.emit(200, &self.tag, &note);
    }
}

impl Drop for StreamAccount {
    fn drop(&mut self) {
        self.finish(true);
    }
}

/// Calls the instructor's upstream and records what the call cost, per the reported usage or an estimate.
struct MeteredClient {
    config: ModelConfig,
    inner: OpenAiCompatibleClient,
    store: Arc<ClassTokenStore>,
    limiter: Arc<ClassLimiter>,
    plain: String,
    logger: Logger,
}

#[async_trait]
impl LlmClient for MeteredClient {
    fn config(&self) -> &ModelConfig {
        &self.config
    }

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let Some(token) = self.store.find(&self.plain) else {
            return Err(LlmError::new("课堂令牌无效").status(401));
        };
        if let Some(denied) = self.store.check(&token) {
            return Err(LlmError {
                provider_code: Some(denied.code),
                ..LlmError::new(denied.message).status(denied.status)
            });
        }
        // The page's class mode queues in the same limiter as `/v1`; the place is given back when
        // this call ends, however it ends.
        let _slot = match self.limiter.acquire(&token.hash) {
            Ok(slot) => slot,
            Err(denied) => {
                return Err(LlmError {
                    provider_code: Some(denied.code),
                    ..LlmError::new(denied.message).status(denied.status)
                });
            }
        };
        let hash = token.hash.clone();
        let started = std::time::Instant::now();
        let tag = token.tag();
        match self.inner.complete(request.clone()).await {
            Ok(response) => {
                let counted = response.usage.map_or_else(
                    || {
                        estimate_messages(&request)
                            + response.text.chars().count() as i64
                            + response
                                .tool_calls
                                .iter()
                                .map(|call| call.arguments_json.chars().count() as i64)
                                .sum::<i64>()
                    },
                    |usage| (usage.prompt_tokens + usage.completion_tokens) as i64,
                );
                self.store.record(&hash, counted);
                (self.logger)(&format!(
                    "WEB 课堂模式 {}ms {tag} tokens={counted}{}",
                    started.elapsed().as_millis(),
                    if response.usage.is_none() {
                        "(估算)"
                    } else {
                        ""
                    }
                ));
                Ok(response)
            }
            Err(error) => {
                self.store.record(&hash, 0);
                (self.logger)(&format!(
                    "WEB 课堂模式 {}ms {tag} upstream_{}",
                    started.elapsed().as_millis(),
                    error
                        .status_code
                        .map_or_else(|| "error".to_string(), |status| status.to_string())
                ));
                // Upstream messages are dropped: they could describe the instructor's account.
                Err(LlmError {
                    message: "模型服务调用失败".to_string(),
                    status_code: error.status_code,
                    retryable: error.retryable,
                    provider_code: error.provider_code,
                })
            }
        }
    }
}

/// About one token per 3 UTF-8 bytes of text plus a fixed cost per image and per message.
fn estimate_messages(request: &ChatRequest) -> i64 {
    request
        .messages
        .iter()
        .map(|message| {
            message
                .parts
                .iter()
                .map(|part| match part {
                    ContentPart::ImagePng(_) => IMAGE_TOKENS,
                    ContentPart::Text(text) => text.len() as i64 / 3,
                })
                .sum::<i64>()
                + 4
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_whitelists_accept_aliases_and_bare_ids() {
        assert_eq!(
            parse_models("xiaoman-class=doubao-seed-1-6, other-id").unwrap(),
            vec![
                ("xiaoman-class".to_string(), "doubao-seed-1-6".to_string()),
                ("other-id".to_string(), "other-id".to_string()),
            ]
        );
        assert!(parse_models("").unwrap().is_empty());
        assert_eq!(
            parse_models("a b c").unwrap().len(),
            3,
            "bare ids may be separated by spaces"
        );
        assert_eq!(parse_models("名字=id").unwrap_err(), "模型白名单格式无效");
        assert_eq!(parse_models("a=").unwrap_err(), "模型白名单格式无效");
    }

    #[test]
    fn env_missing_or_partial_turns_the_feature_off() {
        let none = |_: &str| None;
        assert_eq!(
            ClassOptions::from_env(&none, None).unwrap().1,
            "未配置上游 Key（XIAOMAN_CLASS_UPSTREAM_KEY_FILE）"
        );
        let vars = |name: &str| match name {
            "XIAOMAN_CLASS_UPSTREAM_KEY" => Some("secret".to_string()),
            _ => None,
        };
        assert_eq!(
            ClassOptions::from_env(&vars, None).unwrap().1,
            "未配置模型白名单（XIAOMAN_CLASS_MODELS）"
        );
        let vars = |name: &str| match name {
            "XIAOMAN_CLASS_UPSTREAM_KEY" => Some("secret".to_string()),
            "XIAOMAN_CLASS_MODELS" => Some("xiaoman-class=doubao".to_string()),
            _ => None,
        };
        let (options, status) = ClassOptions::from_env(&vars, None).unwrap();
        let options = options.expect("configured");
        assert_eq!(status, "已开启");
        assert_eq!(options.web_model_name(), "xiaoman-class");
        assert_eq!(options.real_model("xiaoman-class"), Some("doubao"));
        assert_eq!(options.tokens_file, PathBuf::from(DEFAULT_FILE));
    }

    #[test]
    fn estimates_grow_with_the_conversation() {
        let request = ChatRequest::new(vec![xiaoman_api::ChatMessage::user("你好")]);
        let small = estimate_messages(&request);
        let mut messages = request.messages.clone();
        messages.push(xiaoman_api::ChatMessage::user("字".repeat(300)));
        assert!(estimate_messages(&ChatRequest::new(messages)) > small);
    }

    #[test]
    fn the_debug_form_keeps_the_upstream_key_out() {
        let options = ClassOptions::new(
            "file-key-123",
            vec![("xiaoman-class".to_string(), "doubao-real".to_string())],
            "https://upstream.example/api/v3",
            PathBuf::from("/tmp/class-tokens.json"),
        );
        let printed = format!("{options:?}");
        assert!(!printed.contains("file-key-123"), "{printed}");
        assert!(printed.contains("***"), "{printed}");
        assert!(
            printed.contains("xiaoman-class") && !printed.contains("doubao-real"),
            "{printed}"
        );
    }

    /// The limiter and body settings read the Kotlin variables, and a bad number is an error.
    #[test]
    fn the_limiter_and_body_settings_read_their_variables() {
        let vars = |values: Vec<(&'static str, &'static str)>| {
            move |name: &str| {
                values
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            }
        };
        let configured = vec![
            ("XIAOMAN_CLASS_UPSTREAM_KEY", "file-key"),
            ("XIAOMAN_CLASS_MODELS", "xiaoman-class=ep-1"),
            ("XIAOMAN_CLASS_TOKEN_RPM", "12"),
            ("XIAOMAN_CLASS_TOKEN_CONCURRENCY", "3"),
            ("XIAOMAN_CLASS_MAX_CONCURRENT", "7"),
            ("XIAOMAN_CLASS_MAX_BODY_MB", "2"),
        ];
        let (options, status) =
            ClassOptions::from_env(&vars(configured.clone()), None).expect("valid");
        let options = options.expect("configured");
        assert_eq!(status, "已开启");
        assert_eq!(options.per_token_per_minute, 12);
        assert_eq!(options.per_token_concurrency, 3);
        assert_eq!(options.max_concurrent, 7);
        assert_eq!(options.max_body_bytes, 2 * 1024 * 1024);

        let (options, _) = ClassOptions::from_env(
            &vars(vec![
                ("XIAOMAN_CLASS_UPSTREAM_KEY", "k"),
                ("XIAOMAN_CLASS_MODELS", "a=b"),
            ]),
            None,
        )
        .expect("valid");
        let options = options.expect("configured");
        let limits = (
            options.per_token_concurrency,
            options.per_token_per_minute,
            options.max_concurrent,
        );
        assert_eq!(limits, (2, 30, 32), "the Kotlin defaults");
        assert_eq!(options.max_body_bytes, 8 * 1024 * 1024);
        assert_eq!(options.thinking.as_deref(), Some("disabled"));

        for (name, value) in [
            ("XIAOMAN_CLASS_TOKEN_RPM", "0"),
            ("XIAOMAN_CLASS_TOKEN_CONCURRENCY", "-1"),
            ("XIAOMAN_CLASS_MAX_CONCURRENT", "many"),
            ("XIAOMAN_CLASS_MAX_BODY_MB", "abc"),
        ] {
            let mut broken = configured.clone();
            broken.retain(|(key, _)| *key != name);
            broken.push((name, value));
            assert_eq!(
                ClassOptions::from_env(&vars(broken), None).unwrap_err(),
                format!("{name} 需要正整数")
            );
        }
    }
}
