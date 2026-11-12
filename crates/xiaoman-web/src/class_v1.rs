//! The pieces only the `/v1` passthrough needs: the per-token limiter, the request-body limit, the
//! token estimates used when the upstream reports no usage, and the OpenAI-style error envelope.
//!
//! Ported from the Kotlin `ClassGateway`/`ClassLimiter`. The rejection statuses, codes, messages and
//! `Retry-After` values are the ones the Kotlin tests pin.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde_json::{Value, json};

use crate::class_tokens::ClassDenied;
use crate::memory::Clock;

/// Rough cost of one image when the upstream reports no usage.
pub const IMAGE_TOKENS: i64 = 1_000;
/// How much of an oversized body is still read and dropped, so the client sees the error instead of
/// a reset connection.
pub const MAX_DRAIN: u64 = 32 * 1024 * 1024;
/// The rate-limit window, as in the Kotlin limiter.
const WINDOW_MILLIS: i64 = 60_000;

/// Per-token concurrency and requests-per-minute, plus a global concurrency cap.
pub struct ClassLimiter {
    per_token: u32,
    per_minute: u32,
    global: u32,
    clock: Clock,
    inner: Mutex<Limits>,
}

#[derive(Default)]
struct Limits {
    active: HashMap<String, u32>,
    windows: HashMap<String, VecDeque<i64>>,
    total: u32,
}

impl ClassLimiter {
    pub fn new(per_token: u32, per_minute: u32, global: u32, clock: Clock) -> Self {
        Self {
            per_token: per_token.max(1),
            per_minute: per_minute.max(1),
            global: global.max(1),
            clock,
            inner: Mutex::new(Limits::default()),
        }
    }

    /// A limiter slot held until the returned guard is dropped, or why the request must wait.
    pub fn acquire(self: &Arc<Self>, key: &str) -> Result<Slot, ClassDenied> {
        let now = (self.clock)();
        let mut limits = self.lock();
        if let Some(window) = limits.windows.get_mut(key) {
            while window
                .front()
                .is_some_and(|first| now - first >= WINDOW_MILLIS)
            {
                window.pop_front();
            }
        }
        if limits.active.get(key).copied().unwrap_or(0) >= self.per_token {
            return Err(ClassDenied::new(
                429,
                "too_many_concurrent_requests",
                "这个课堂令牌同时进行的请求太多，请等上一个完成",
                Some(1),
            ));
        }
        if limits.total >= self.global {
            return Err(ClassDenied::new(
                429,
                "server_busy",
                "课堂服务正忙，请稍后重试",
                Some(2),
            ));
        }
        let window = limits.windows.entry(key.to_string()).or_default();
        if window.len() as u32 >= self.per_minute {
            let first = window.front().copied().unwrap_or(now);
            let wait = ((first + WINDOW_MILLIS - now + 999) / 1000).max(1);
            return Err(ClassDenied::new(
                429,
                "rate_limit_exceeded",
                format!("请求太频繁：每个令牌每分钟最多 {} 次", self.per_minute),
                Some(wait),
            ));
        }
        window.push_back(now);
        limits.total += 1;
        *limits.active.entry(key.to_string()).or_insert(0) += 1;
        Ok(Slot {
            limiter: self.clone(),
            key: key.to_string(),
            released: false,
        })
    }

    fn release(&self, key: &str) {
        let mut limits = self.lock();
        limits.total = limits.total.saturating_sub(1);
        let left = limits
            .active
            .get(key)
            .copied()
            .unwrap_or(1)
            .saturating_sub(1);
        if left == 0 {
            limits.active.remove(key);
        } else {
            limits.active.insert(key.to_string(), left);
        }
    }

    fn lock(&self) -> MutexGuard<'_, Limits> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// One admitted request's place in the limiter; dropping it gives the place back.
pub struct Slot {
    limiter: Arc<ClassLimiter>,
    key: String,
    released: bool,
}

impl Slot {
    /// Gives the place back; releasing twice does nothing.
    pub fn release(&mut self) {
        if !self.released {
            self.released = true;
            self.limiter.release(&self.key);
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.release();
    }
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Slot")
            .field("released", &self.released)
            .finish()
    }
}

/// `total_tokens`, or prompt plus completion when the upstream reports only those.
pub fn usage_of(value: &Value) -> Option<i64> {
    let usage = value.get("usage")?.as_object()?;
    let number = |key: &str| usage.get(key).and_then(Value::as_i64);
    if let Some(total) = number("total_tokens") {
        return Some(total);
    }
    let parts: Vec<i64> = ["prompt_tokens", "completion_tokens"]
        .into_iter()
        .filter_map(number)
        .collect();
    (!parts.is_empty()).then(|| parts.iter().sum())
}

/// About one token per 3 UTF-8 bytes of text (roughly one per Chinese character) plus a fixed cost
/// per image and per message.
pub fn estimate_prompt(root: &Value) -> i64 {
    let messages = root.get("messages").and_then(Value::as_array);
    messages.map_or(0, |messages| {
        messages
            .iter()
            .map(|message| match message.get("content") {
                Some(Value::String(text)) => text.len() as i64 / 3 + 4,
                Some(Value::Array(parts)) => {
                    parts
                        .iter()
                        .map(|part| {
                            if part.get("type").and_then(Value::as_str) == Some("image_url") {
                                IMAGE_TOKENS
                            } else {
                                part.get("text")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .len() as i64
                                    / 3
                            }
                        })
                        .sum::<i64>()
                        + 4
                }
                _ => 4,
            })
            .sum()
    })
}

/// Output characters in a completion or in one stream chunk.
pub fn estimate_completion(value: Option<&Value>) -> i64 {
    let choices = value
        .and_then(|value| value.get("choices"))
        .and_then(Value::as_array);
    choices.map_or(0, |choices| {
        choices
            .iter()
            .map(|choice| {
                let message = choice
                    .get("message")
                    .filter(|message| message.is_object())
                    .or_else(|| choice.get("delta").filter(|delta| delta.is_object()));
                let Some(message) = message else {
                    return 0;
                };
                let count = |key: &str| {
                    message
                        .get(key)
                        .and_then(Value::as_str)
                        .map_or(0, |text| text.chars().count() as i64)
                };
                let arguments: i64 =
                    message
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .map_or(0, |calls| {
                            calls
                                .iter()
                                .map(|call| {
                                    call.get("function")
                                        .and_then(|function| function.get("arguments"))
                                        .and_then(Value::as_str)
                                        .map_or(0, |arguments| arguments.chars().count() as i64)
                                })
                                .sum()
                        });
                count("content") + count("reasoning_content") + arguments
            })
            .sum()
    })
}

/// The OpenAI-style error every `/v1` failure returns, with `Retry-After` when there is one.
pub fn error_response(denied: &ClassDenied) -> Response {
    let kind = match denied.status {
        401 => "authentication_error",
        429 if denied.code.contains("quota") => "insufficient_quota",
        429 => "rate_limit_error",
        400..=499 => "invalid_request_error",
        _ => "api_error",
    };
    let body = json!({
        "error": { "message": &denied.message, "type": kind, "code": &denied.code },
    })
    .to_string();
    let status = StatusCode::from_u16(denied.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (
        status,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        body,
    )
        .into_response();
    if let Some(seconds) = denied.retry_after_seconds {
        if let Ok(value) = header::HeaderValue::from_str(&seconds.to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
    }
    response
}

/// The client-facing error for a failed upstream call; it never carries the instructor's key.
pub fn upstream_error(status: u16, body: &str, key: &str) -> ClassDenied {
    let root: Option<Value> = serde_json::from_str(body).ok();
    let error = root
        .as_ref()
        .and_then(|root| root.get("error"))
        .filter(|error| error.is_object());
    let code = error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .filter(|code| !code.trim().is_empty())
        .or_else(|| {
            root.as_ref()
                .and_then(|root| root.get("code"))
                .and_then(Value::as_str)
        })
        .filter(|code| !code.trim().is_empty())
        .filter(|code| is_safe_code(code) && !(code.contains(key) && !key.is_empty()))
        .map(str::to_string);
    let message: String = redact(
        error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .or_else(|| {
                root.as_ref()
                    .and_then(|root| root.get("message"))
                    .and_then(Value::as_str)
            })
            .unwrap_or_default(),
        key,
    )
    .chars()
    .take(300)
    .collect();
    match () {
        _ if code.as_deref() == Some("SetLimitExceeded") => ClassDenied::new(
            429,
            "upstream_quota_exceeded",
            "讲师账号的模型限额已用完，请联系讲师",
            None,
        ),
        _ if status == 401 || status == 403 => ClassDenied::new(
            502,
            "upstream_auth_failed",
            "讲师的模型账号认证失败，请联系讲师",
            None,
        ),
        _ if status == 404 => ClassDenied::new(
            502,
            "upstream_model_unavailable",
            "模型服务上找不到这个模型，请联系讲师",
            None,
        ),
        _ if status == 429 => ClassDenied::new(
            429,
            code.unwrap_or_else(|| "upstream_rate_limited".to_string()),
            with_detail("模型服务限流，请稍后重试", &message),
            Some(2),
        ),
        _ if (400..=499).contains(&status) => ClassDenied::new(
            400,
            code.unwrap_or_else(|| "invalid_request_error".to_string()),
            with_detail("模型服务拒绝了请求", &message),
            None,
        ),
        _ => ClassDenied::new(
            502,
            "upstream_error",
            format!("模型服务暂时出错（{status}），请稍后重试"),
            None,
        ),
    }
}

/// An error code the client may see: a short identifier, never free text.
fn is_safe_code(code: &str) -> bool {
    let mut characters = code.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && code.chars().count() <= 64
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

fn with_detail(base: &str, detail: &str) -> String {
    if detail.is_empty() {
        base.to_string()
    } else {
        format!("{base}：{detail}")
    }
}

/// Hides the instructor's key wherever the upstream echoed it.
pub fn redact(text: &str, key: &str) -> String {
    if key.is_empty() {
        return text.to_string();
    }
    text.replace(key, "***")
}

/// The bearer token of an `Authorization: Bearer <token>` header.
pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let bytes = value.as_bytes();
    if bytes.len() <= 7 || !bytes[..7].eq_ignore_ascii_case(b"Bearer ") {
        return None;
    }
    Some(value[7..].trim())
}

/// The `Content-Length` a client declared, when it sent a usable one.
pub fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// What reading a request body produced.
pub enum BodyRead {
    Body(Bytes),
    /// Above the limit; the rest was read and dropped.
    TooLarge,
    /// The client broke off mid-body.
    Failed,
}

/// Reads the request body up to `max` bytes. A body above the limit is drained before the error is
/// returned, so the client reads the reply instead of a reset connection.
pub async fn read_body(body: Body, declared: Option<u64>, max: u64) -> BodyRead {
    let mut stream = Box::pin(body.into_data_stream());
    if declared.is_some_and(|length| length > max) {
        drain(&mut stream, MAX_DRAIN).await;
        return BodyRead::TooLarge;
    }
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        match stream.next().await {
            None => return BodyRead::Body(Bytes::from(buffer)),
            Some(Err(_)) => return BodyRead::Failed,
            Some(Ok(chunk)) => {
                buffer.extend_from_slice(&chunk);
                if buffer.len() as u64 > max {
                    drain(&mut stream, MAX_DRAIN).await;
                    return BodyRead::TooLarge;
                }
            }
        }
    }
}

/// Reads and drops a rejected body up to `limit` bytes, so the client sees the error.
pub async fn drain_body(body: Body, limit: u64) {
    let mut stream = Box::pin(body.into_data_stream());
    drain(&mut stream, limit).await;
}

async fn drain<S>(stream: &mut S, limit: u64)
where
    S: futures::Stream<Item = Result<Bytes, axum::Error>> + Unpin,
{
    let mut left = limit;
    while left > 0 {
        match stream.next().await {
            Some(Ok(chunk)) => left = left.saturating_sub(chunk.len() as u64),
            _ => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;

    fn clock_at(now: i64) -> (Clock, Arc<AtomicI64>) {
        let ticks = Arc::new(AtomicI64::new(now));
        let clock: Clock = {
            let ticks = ticks.clone();
            Arc::new(move || ticks.load(Ordering::SeqCst))
        };
        (clock, ticks)
    }

    #[test]
    fn the_limiter_rejects_beyond_its_limits() {
        let (clock, _ticks) = clock_at(1_000_000);
        let limiter = Arc::new(ClassLimiter::new(1, 100, 2, clock));
        let mut first = limiter.acquire("a").expect("the first request of a");
        assert_eq!(
            limiter.acquire("a").unwrap_err().code,
            "too_many_concurrent_requests"
        );
        let second = limiter.acquire("b").expect("the first request of b");
        assert_eq!(limiter.acquire("c").unwrap_err().code, "server_busy");
        // Releasing twice is harmless, and the place is free again afterwards.
        first.release();
        first.release();
        assert!(limiter.acquire("c").is_ok(), "a released slot is usable");
        drop(second);
        assert!(
            limiter.acquire("a").is_ok(),
            "dropping the guard releases it"
        );
    }

    #[test]
    fn the_minute_window_reports_how_long_to_wait() {
        let (clock, ticks) = clock_at(0);
        let limiter = Arc::new(ClassLimiter::new(2, 2, 10, clock));
        limiter.acquire("a").expect("first").release();
        limiter.acquire("a").expect("second").release();
        let denied = limiter.acquire("a").unwrap_err();
        assert_eq!(
            (denied.status, denied.code.as_str()),
            (429, "rate_limit_exceeded")
        );
        assert_eq!(denied.message, "请求太频繁：每个令牌每分钟最多 2 次");
        assert_eq!(denied.retry_after_seconds, Some(60));
        ticks.store(30_000, Ordering::SeqCst);
        assert_eq!(
            limiter.acquire("a").unwrap_err().retry_after_seconds,
            Some(30)
        );
        ticks.store(60_000, Ordering::SeqCst);
        assert!(limiter.acquire("a").is_ok(), "the window has emptied");
        assert!(
            limiter.acquire("b").is_ok(),
            "other tokens are not affected"
        );
    }

    #[test]
    fn usage_and_estimates_follow_the_kotlin_rules() {
        let completion = json!({
            "choices": [{
                "message": { "role": "assistant", "content": "你好，我是小满" },
                "finish_reason": "stop",
            }],
            "usage": { "prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120 },
        });
        assert_eq!(usage_of(&completion), Some(120));
        assert_eq!(estimate_completion(Some(&completion)), 7);
        let chunk = json!({
            "choices": [{ "delta": { "content": "好", "reasoning_content": "想" } }],
            "usage": { "prompt_tokens": 3, "completion_tokens": 4 },
        });
        assert_eq!(
            usage_of(&chunk),
            Some(7),
            "prompt plus completion when there is no total"
        );
        assert_eq!(estimate_completion(Some(&chunk)), 2);
        let tools = json!({ "choices": [{ "delta": { "tool_calls": [{ "function": { "arguments": "{}" } }] } }] });
        assert_eq!(estimate_completion(Some(&tools)), 2);
        assert_eq!(estimate_completion(None), 0);
        assert_eq!(usage_of(&json!({ "choices": [] })), None);

        let prompt = json!({ "messages": [
            { "role": "user", "content": "你好" },
            { "role": "user", "content": [
                { "type": "text", "text": "看截图" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } },
            ] },
        ] });
        assert_eq!(
            estimate_prompt(&prompt),
            6 + 3 + IMAGE_TOKENS + 4,
            "2 + 4 for 你好, then 3 + an image + 4"
        );
        assert_eq!(estimate_prompt(&json!({})), 0);
    }

    #[tokio::test]
    async fn the_error_envelope_matches_the_openai_shape() {
        async fn read(denied: &ClassDenied) -> (StatusCode, HeaderMap, Value) {
            let response = error_response(denied);
            let status = response.status();
            let headers = response.headers().clone();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("a body");
            (
                status,
                headers,
                serde_json::from_slice(&bytes).expect("json"),
            )
        }
        let (status, headers, body) = read(&ClassDenied::new(
            401,
            "invalid_token",
            "课堂令牌无效",
            None,
        ))
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "invalid_token");
        assert_eq!(body["error"]["type"], "authentication_error");
        assert_eq!(body["error"]["message"], "课堂令牌无效");
        assert!(headers.get(header::RETRY_AFTER).is_none());

        let (status, headers, body) = read(&ClassDenied::new(
            429,
            "daily_quota_exceeded",
            "额度",
            Some(3),
        ))
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["error"]["type"], "insufficient_quota");
        assert_eq!(headers[header::RETRY_AFTER], "3");

        let (_, _, body) = read(&ClassDenied::new(429, "rate_limit_exceeded", "忙", Some(1))).await;
        assert_eq!(body["error"]["type"], "rate_limit_error");
        let (status, _, body) = read(&ClassDenied::new(404, "not_found", "接口不存在", None)).await;
        assert_eq!(
            (status, body["error"]["type"].as_str()),
            (StatusCode::NOT_FOUND, Some("invalid_request_error"))
        );
        let (_, _, body) = read(&ClassDenied::new(500, "server_error", "失败", None)).await;
        assert_eq!(body["error"]["type"], "api_error");
    }

    #[test]
    fn upstream_errors_map_to_the_kotlin_statuses_without_the_key() {
        const KEY: &str = "ark-instructor-secret";
        let broken = upstream_error(
            400,
            r#"{"error":{"code":"InvalidParameter","message":"image too large; header was Bearer ark-instructor-secret"}}"#,
            KEY,
        );
        assert_eq!(
            (broken.status, broken.code.as_str()),
            (400, "InvalidParameter")
        );
        assert!(
            broken
                .message
                .starts_with("模型服务拒绝了请求：image too large")
        );
        assert!(!broken.message.contains(KEY), "{}", broken.message);

        let auth = upstream_error(
            401,
            r#"{"error":{"code":"AuthenticationError","message":"bad key"}}"#,
            KEY,
        );
        assert_eq!(
            (auth.status, auth.code.as_str()),
            (502, "upstream_auth_failed")
        );
        assert_eq!(upstream_error(403, "", KEY).code, "upstream_auth_failed");
        let missing = upstream_error(404, "", KEY);
        assert_eq!(
            (missing.status, missing.code.as_str()),
            (502, "upstream_model_unavailable")
        );
        let quota = upstream_error(400, r#"{"error":{"code":"SetLimitExceeded"}}"#, KEY);
        assert_eq!(
            (quota.status, quota.code.as_str()),
            (429, "upstream_quota_exceeded")
        );
        let limited = upstream_error(
            429,
            r#"{"error":{"code":"RateLimitExceeded","message":"slow down"}}"#,
            KEY,
        );
        assert_eq!(
            (limited.status, limited.code.as_str()),
            (429, "RateLimitExceeded")
        );
        assert_eq!(limited.message, "模型服务限流，请稍后重试：slow down");
        assert_eq!(limited.retry_after_seconds, Some(2));
        let plain = upstream_error(429, "", KEY);
        assert_eq!(plain.code, "upstream_rate_limited");
        let broken = upstream_error(500, "not json", KEY);
        assert_eq!(
            (broken.status, broken.code.as_str()),
            (502, "upstream_error")
        );
        assert_eq!(broken.message, "模型服务暂时出错（500），请稍后重试");
        // A code that echoes the key, or is not an identifier, never reaches the client.
        let leaky = upstream_error(400, &format!(r#"{{"error":{{"code":"{KEY}"}}}}"#), KEY);
        assert_eq!(
            (leaky.status, leaky.code.as_str()),
            (400, "invalid_request_error")
        );
        let odd = upstream_error(400, r#"{"error":{"code":"图片太大"}}"#, KEY);
        assert_eq!(odd.code, "invalid_request_error");
        let long = upstream_error(
            400,
            &format!(r#"{{"error":{{"message":"{}"}}}}"#, "a".repeat(400)),
            KEY,
        );
        assert_eq!(
            long.message.chars().count(),
            "模型服务拒绝了请求：".chars().count() + 300
        );
    }

    #[tokio::test]
    async fn bodies_above_the_limit_are_read_and_dropped() {
        let max = 8;
        assert!(matches!(
            read_body(Body::from("12345678"), None, max).await,
            BodyRead::Body(body) if body == "12345678"
        ));
        assert!(matches!(
            read_body(Body::from("123456789"), None, max).await,
            BodyRead::TooLarge
        ));
        // A declared length above the limit is refused before the body is read, but still drained.
        assert!(matches!(
            read_body(Body::from("123456789"), Some(9), max).await,
            BodyRead::TooLarge
        ));
        assert!(
            matches!(read_body(Body::from("123"), None, max).await, BodyRead::Body(body) if body == "123")
        );
        drain_body(Body::from("123456789"), 4).await;
    }

    #[test]
    fn the_bearer_header_is_read_like_the_kotlin_gateway() {
        let mut headers = HeaderMap::new();
        assert!(bearer(&headers).is_none());
        headers.insert(header::AUTHORIZATION, "xm-token".parse().unwrap());
        assert!(bearer(&headers).is_none(), "no scheme");
        headers.insert(header::AUTHORIZATION, "Bearer ".parse().unwrap());
        assert!(bearer(&headers).is_none(), "no token");
        headers.insert(
            header::AUTHORIZATION,
            "  bearer   xm-abc  ".parse().unwrap(),
        );
        assert_eq!(bearer(&headers), Some("xm-abc"));
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_bytes("Bearer 令牌".as_bytes()).expect("raw bytes"),
        );
        assert!(
            bearer(&headers).is_none(),
            "a value that is not visible ASCII is ignored"
        );
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, "12".parse().unwrap());
        assert_eq!(content_length(&headers), Some(12));
        headers.insert(header::CONTENT_LENGTH, "abc".parse().unwrap());
        assert_eq!(content_length(&headers), None);
    }

    #[test]
    fn redaction_hides_the_key_and_leaves_other_text_alone() {
        assert_eq!(redact("key ark-1 here", "ark-1"), "key *** here");
        assert_eq!(redact("nothing", ""), "nothing");
    }
}
