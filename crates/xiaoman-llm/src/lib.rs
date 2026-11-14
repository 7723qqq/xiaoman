//! `LlmClient` for any OpenAI-compatible `/chat/completions` endpoint.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde_json::{Map, Value, json};
use xiaoman_api::{
    ChatMessage, ChatRequest, ChatResponse, ChatRole, ContentPart, LlmClient, LlmError,
    ModelConfig, ToolCall, Usage,
};

pub type SleepFn = Arc<dyn Fn(u64) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub struct ClientOptions {
    pub max_retries: u32,
    pub base_delay_millis: u64,
    /// Backoff sleep; replace in tests to avoid real waiting.
    pub sleep: SleepFn,
    pub http: Option<reqwest::Client>,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay_millis: 500,
            sleep: Arc::new(|millis| Box::pin(tokio::time::sleep(Duration::from_millis(millis)))),
            http: None,
        }
    }
}

/// Retries 408/409/429/5xx and transport errors up to `max_retries` times with exponential backoff
/// starting at `base_delay_millis` (or the server's `Retry-After` seconds). If the endpoint rejects
/// `response_format`, the request is retried once without it and JSON mode stays off for this client.
/// Error messages never contain the API key.
pub struct OpenAiCompatibleClient {
    config: ModelConfig,
    http: reqwest::Client,
    url: String,
    max_retries: u32,
    base_delay_millis: u64,
    sleep: SleepFn,
    json_mode_unsupported: AtomicBool,
}

impl OpenAiCompatibleClient {
    pub fn new(config: ModelConfig) -> Self {
        Self::with_options(config, ClientOptions::default())
    }

    pub fn with_options(config: ModelConfig, options: ClientOptions) -> Self {
        let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
        let http = options.http.unwrap_or_else(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(config.timeout_seconds.max(1) as u64))
                .build()
                .unwrap_or_default()
        });
        Self {
            config,
            http,
            url,
            max_retries: options.max_retries,
            base_delay_millis: options.base_delay_millis,
            sleep: options.sleep,
            json_mode_unsupported: AtomicBool::new(false),
        }
    }

    /// `GET {base_url}/models`: the endpoint's model ids, deduplicated in server order.
    ///
    /// One attempt, no retries: the Settings screen calls it from a button, so a failure should
    /// surface as it happens instead of after a backoff. Error messages never contain the API key.
    pub async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let url = format!("{}/models", self.config.base_url.trim_end_matches('/'));
        let mut request = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key));
        for (name, value) in &self.config.extra_headers {
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(|error| LlmError {
            message: self.redact(&format!("Request to {url} failed: {error}")),
            status_code: None,
            retryable: true,
            provider_code: None,
        })?;
        let code = response.status().as_u16();
        let body = response.text().await.map_err(|error| LlmError {
            message: self.redact(&format!("Reading {url} failed: {error}")),
            status_code: Some(code),
            retryable: true,
            provider_code: None,
        })?;
        if !(200..=299).contains(&code) {
            return Err(LlmError {
                message: self.redact(&format!(
                    "HTTP {code} from {url}: {}",
                    self.error_detail(&body)
                )),
                status_code: Some(code),
                retryable: is_retryable(code),
                provider_code: self.provider_code(&body),
            });
        }
        let root: Value = serde_json::from_str(&body).map_err(|_| LlmError {
            message: self.redact(&format!(
                "Malformed response from {url}: {}",
                truncate(&body, 200)
            )),
            status_code: Some(code),
            retryable: false,
            provider_code: None,
        })?;
        Ok(model_ids(root.get("data").and_then(Value::as_array)))
    }

    pub fn build_body(&self, request: &ChatRequest, json_mode: bool) -> Value {
        let mut body = Map::new();
        body.insert("model".into(), json!(self.config.model));
        body.insert(
            "messages".into(),
            Value::Array(request.messages.iter().map(encode_message).collect()),
        );
        if !request.tools.is_empty() {
            let tools = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        }
                    })
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
        }
        if json_mode {
            body.insert("response_format".into(), json!({"type": "json_object"}));
        }
        if let Some(temperature) = self.config.temperature {
            body.insert("temperature".into(), json!(temperature));
        }
        if let Some(max_tokens) = self.config.max_tokens {
            body.insert("max_tokens".into(), json!(max_tokens));
        }
        Value::Object(body)
    }

    fn parse_response(&self, body: &str) -> Result<ChatResponse, LlmError> {
        let root: Value = serde_json::from_str(body).map_err(|_| LlmError {
            message: self.redact(&format!(
                "Malformed response from {}: {}",
                self.url,
                truncate(body, 200)
            )),
            status_code: Some(200),
            retryable: false,
            provider_code: None,
        })?;
        let choice = root
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .ok_or_else(|| LlmError {
                message: self.redact(&format!(
                    "Response from {} has no choices: {}",
                    self.url,
                    error_detail(body)
                )),
                status_code: Some(200),
                retryable: false,
                provider_code: None,
            })?;
        let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));
        let tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|call| {
                        let function = call.get("function")?;
                        let arguments = match function.get("arguments") {
                            None | Some(Value::Null) => "{}".to_string(),
                            Some(Value::String(text)) => text.clone(),
                            Some(other) => other.to_string(),
                        };
                        Some(ToolCall {
                            id: call
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            name: function
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            arguments_json: arguments,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let usage = root
            .get("usage")
            .and_then(Value::as_object)
            .map(|usage| Usage {
                prompt_tokens: integer(usage, "prompt_tokens"),
                completion_tokens: integer(usage, "completion_tokens"),
            });
        Ok(ChatResponse {
            text: content_text(message.get("content")),
            tool_calls,
            finish_reason: choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .map(str::to_string),
            usage,
        })
    }

    fn backoff(&self, attempt: u32, retry_after: Option<&str>) -> u64 {
        if let Some(seconds) = retry_after
            .and_then(|value| value.trim().parse::<i64>().ok())
            .filter(|v| *v >= 0)
        {
            return seconds as u64 * 1000;
        }
        self.base_delay_millis << attempt.min(20)
    }

    fn error_detail(&self, body: &str) -> String {
        error_detail(body)
    }

    fn provider_code(&self, body: &str) -> Option<String> {
        let root: Value = serde_json::from_str(body).ok()?;
        let error = root.get("error");
        let code = error
            .and_then(|error| error.get("code"))
            .or_else(|| root.get("code"))
            .and_then(Value::as_str)?;
        if !is_provider_code(code) {
            return None;
        }
        if !self.config.api_key.is_empty() && code.contains(&self.config.api_key) {
            return None;
        }
        Some(code.to_string())
    }

    fn redact(&self, text: &str) -> String {
        if self.config.api_key.is_empty() {
            text.to_string()
        } else {
            text.replace(&self.config.api_key, "***")
        }
    }

    async fn send(&self, body: &Value) -> Result<HttpResult, String> {
        let mut request = self
            .http
            .post(&self.url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json; charset=utf-8")
            .body(body.to_string());
        for (name, value) in &self.config.extra_headers {
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        let code = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let text = response.text().await.map_err(|error| error.to_string())?;
        Ok(HttpResult {
            code,
            body: text,
            retry_after,
        })
    }
}

#[async_trait]
impl LlmClient for OpenAiCompatibleClient {
    fn config(&self) -> &ModelConfig {
        &self.config
    }

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let mut attempt: u32 = 0;
        let mut json_mode = request.json_mode && !self.json_mode_unsupported.load(Ordering::SeqCst);
        loop {
            let body = self.build_body(&request, json_mode);
            let result = match self.send(&body).await {
                Ok(result) => result,
                Err(message) => {
                    if attempt < self.max_retries {
                        let delay = self.backoff(attempt, None);
                        attempt += 1;
                        (self.sleep)(delay).await;
                        continue;
                    }
                    return Err(LlmError {
                        message: self.redact(&format!("Request to {} failed: {message}", self.url)),
                        status_code: None,
                        retryable: true,
                        provider_code: None,
                    });
                }
            };
            if (200..=299).contains(&result.code) {
                return self.parse_response(&result.body);
            }
            if result.code == 400 && json_mode && result.body.contains("response_format") {
                self.json_mode_unsupported.store(true, Ordering::SeqCst);
                json_mode = false;
                continue;
            }
            let provider_code = self.provider_code(&result.body);
            // A configured spending/inference limit needs an account change, not backoff.
            let retryable =
                is_retryable(result.code) && provider_code.as_deref() != Some("SetLimitExceeded");
            if retryable && attempt < self.max_retries {
                let delay = self.backoff(attempt, result.retry_after.as_deref());
                attempt += 1;
                (self.sleep)(delay).await;
                continue;
            }
            return Err(LlmError {
                message: self.redact(&format!(
                    "HTTP {} from {}: {}",
                    result.code,
                    self.url,
                    self.error_detail(&result.body)
                )),
                status_code: Some(result.code),
                retryable,
                provider_code,
            });
        }
    }
}

struct HttpResult {
    code: u16,
    body: String,
    retry_after: Option<String>,
}

fn encode_message(message: &ChatMessage) -> Value {
    let mut encoded = Map::new();
    encoded.insert("role".into(), json!(role_name(message.role)));
    let has_image = message
        .parts
        .iter()
        .any(|part| matches!(part, ContentPart::ImagePng(_)));
    if has_image {
        let parts = message
            .parts
            .iter()
            .map(|part| match part {
                ContentPart::Text(text) => json!({"type": "text", "text": text}),
                ContentPart::ImagePng(bytes) => json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
                    }
                }),
            })
            .collect();
        encoded.insert("content".into(), Value::Array(parts));
    } else if !message.tool_calls.is_empty() && message.text().is_empty() {
        encoded.insert("content".into(), Value::Null);
    } else {
        encoded.insert("content".into(), json!(message.text()));
    }
    if message.role == ChatRole::Assistant && !message.tool_calls.is_empty() {
        let calls = message
            .tool_calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": {"name": call.name, "arguments": call.arguments_json}
                })
            })
            .collect();
        encoded.insert("tool_calls".into(), Value::Array(calls));
    }
    if let Some(call_id) = &message.tool_call_id {
        encoded.insert("tool_call_id".into(), json!(call_id));
    }
    Value::Object(encoded)
}

fn role_name(role: ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::Tool => "tool",
    }
}

fn content_text(content: Option<&Value>) -> String {
    match content {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .map(|part| match part {
                Value::String(text) => text.clone(),
                Value::Object(object) => match object.get("type").and_then(Value::as_str) {
                    None | Some("text") => object
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    _ => String::new(),
                },
                _ => String::new(),
            })
            .collect(),
        Some(Value::Object(object)) => object
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Some(other) => other.to_string(),
    }
}

/// `data[].id` of a `/models` reply, deduplicated in server order. `name`/`model` are accepted
/// for compatible servers that label the field differently; entries without either are skipped.
fn model_ids(data: Option<&Vec<Value>>) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for entry in data.into_iter().flatten() {
        let id = ["id", "name", "model"]
            .iter()
            .find_map(|key| entry.get(*key).and_then(Value::as_str))
            .unwrap_or_default();
        if !id.is_empty() && !ids.iter().any(|existing| existing == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

fn integer(map: &Map<String, Value>, key: &str) -> i32 {
    map.get(key).and_then(Value::as_i64).unwrap_or_default() as i32
}

fn is_retryable(code: u16) -> bool {
    matches!(code, 408 | 409 | 429) || (500..=599).contains(&code)
}

fn is_provider_code(code: &str) -> bool {
    let mut chars = code.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    let rest = chars.collect::<Vec<_>>();
    if rest.len() > 127 {
        return false;
    }
    rest.iter()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn error_detail(body: &str) -> String {
    let message =
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|root| match root.get("error") {
                Some(Value::Object(object)) => object
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                Some(Value::String(text)) => Some(text.clone()),
                _ => root
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
    let detail = truncate(&message.unwrap_or_else(|| body.to_string()), 500);
    if detail.trim().is_empty() {
        "<empty body>".to_string()
    } else {
        detail
    }
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Builds [`OpenAiCompatibleClient`]s that share one connection pool.
pub struct LlmClients {
    http: reqwest::Client,
    max_retries: u32,
}

impl Default for LlmClients {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmClients {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            max_retries: 2,
        }
    }

    pub fn with_retries(max_retries: u32) -> Self {
        Self {
            http: reqwest::Client::new(),
            max_retries,
        }
    }

    pub fn create(&self, config: ModelConfig) -> OpenAiCompatibleClient {
        OpenAiCompatibleClient::with_options(
            config,
            ClientOptions {
                max_retries: self.max_retries,
                http: Some(self.http.clone()),
                ..Default::default()
            },
        )
    }
}
