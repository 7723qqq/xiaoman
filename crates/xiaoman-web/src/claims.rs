//! Self-service class tokens (`自助领取`): a student proves they own a forum account by replying a
//! one-time phrase in the instructor's forum topic, and gets one token per forum account.
//!
//! Off unless a topic id and an expiry are configured. [`ClaimService::start`] checks the forum
//! account and hands out a phrase; [`ClaimService::verify`] looks for the account's reply containing
//! it and writes a new token into the class-token file. Plaintext tokens are returned once and never
//! logged; logs carry `论坛:用户名(xm-xxx)` only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::class_tokens::{
    ClassToken, ClassTokenCodec, ClassTokenStore, ForumClaim, display, parse_day_start,
    parse_moment,
};
use crate::memory::Clock;
use crate::random;
use crate::server::Logger;

/// What a claimant must reply, followed by a one-time code.
pub const PHRASE: &str = "领取小满令牌";
/// Label prefix of tokens issued to a forum account: `论坛:Alice`.
pub const LABEL: &str = "论坛:";
/// No 0/1/I/L/O, so a code read off a phone screen is unambiguous.
const CODE: &str = "23456789ABCDEFGHJKMNPQRSTUVWXYZ";
/// How many posts one `posts.json` call fetches.
const BATCH: usize = 50;
/// Students check within the same minute, so the topic stream is shared for a few seconds.
const STREAM_CACHE_MILLIS: i64 = 3_000;
/// Fetched posts are reused a little longer.
const POST_CACHE_MILLIS: i64 = 30_000;

/// `ClaimConfig` of the Kotlin server: everything `/claim` needs, read from `XIAOMAN_CLAIM_*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimConfig {
    pub topic_id: i64,
    /// Every claimed token expires at this moment (the end of the last class).
    pub expires_at: i64,
    pub max: usize,
    pub total_tokens: i64,
    pub daily_tokens: i64,
    pub forum_url: String,
    /// Only accounts registered before this moment may claim; `None` allows any.
    pub created_before: Option<i64>,
    pub min_trust: i32,
    pub challenge_millis: i64,
    pub per_ip_starts_per_minute: usize,
    pub per_ip_checks_per_minute: usize,
    pub global_starts_per_minute: usize,
    pub global_checks_per_minute: usize,
    pub max_pending: usize,
    /// How many of the topic's newest posts a check reads at most.
    pub scan_posts: usize,
    pub forum_timeout_seconds: u64,
    pub forum_concurrency: usize,
}

impl ClaimConfig {
    pub const DEFAULT_FORUM: &'static str = "https://forum.trae.cn";

    /// The topic and the moment the class ends; everything else keeps the Kotlin defaults.
    pub fn new(topic_id: i64, expires_at: i64) -> Self {
        Self {
            topic_id,
            expires_at,
            max: 50,
            total_tokens: 1_000_000,
            daily_tokens: 1_000_000,
            forum_url: Self::DEFAULT_FORUM.to_string(),
            created_before: None,
            min_trust: 0,
            challenge_millis: 10 * 60_000,
            per_ip_starts_per_minute: 5,
            per_ip_checks_per_minute: 20,
            global_starts_per_minute: 120,
            global_checks_per_minute: 120,
            max_pending: 5_000,
            scan_posts: 300,
            forum_timeout_seconds: 10,
            forum_concurrency: 4,
        }
    }

    /// `daily_tokens` defaults to the same amount as `total_tokens`.
    pub fn with_totals(mut self, total_tokens: i64) -> Self {
        self.total_tokens = total_tokens;
        self.daily_tokens = total_tokens;
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.topic_id <= 0
            || self.max == 0
            || self.total_tokens <= 0
            || self.daily_tokens <= 0
            || !(0..=4).contains(&self.min_trust)
        {
            return Err("领取帖、份数、额度或最低信任等级无效".to_string());
        }
        if self.challenge_millis <= 0
            || self.per_ip_starts_per_minute == 0
            || self.per_ip_checks_per_minute == 0
            || self.global_starts_per_minute == 0
            || self.global_checks_per_minute == 0
        {
            return Err("自助领取的时限或频率限制无效".to_string());
        }
        if self.max_pending == 0
            || self.scan_posts == 0
            || self.forum_timeout_seconds == 0
            || self.forum_concurrency == 0
        {
            return Err("自助领取的论坛读取设置无效".to_string());
        }
        forum_base(&self.forum_url)?;
        Ok(())
    }

    pub fn topic_url(&self) -> String {
        format!(
            "{}/t/{}",
            self.forum_url.trim().trim_end_matches('/'),
            self.topic_id
        )
    }

    /// Reads `XIAOMAN_CLAIM_*`; `None` (off) with the reason when no topic is configured. Invalid
    /// values are an error, so a typo never silently turns claiming off.
    pub fn from_env(
        env: &dyn Fn(&str) -> Option<String>,
        offset_minutes: i32,
        now: i64,
    ) -> Result<(Option<ClaimConfig>, String), String> {
        let value = |name: &str| {
            env(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let long = |name: &str, default: i64| -> Result<i64, String> {
            match value(name) {
                Some(text) => {
                    let digits: String = text
                        .chars()
                        .filter(|character| *character != '_' && *character != ',')
                        .collect();
                    digits
                        .parse::<i64>()
                        .ok()
                        .filter(|number| *number > 0)
                        .ok_or_else(|| format!("{name} 需要正整数"))
                }
                None => Ok(default),
            }
        };
        let Some(topic) = value("XIAOMAN_CLAIM_TOPIC_ID") else {
            return Ok((None, "未配置领取帖（XIAOMAN_CLAIM_TOPIC_ID）".to_string()));
        };
        let topic_id = Self::parse_topic(&topic)
            .ok_or_else(|| "XIAOMAN_CLAIM_TOPIC_ID 需要帖子编号或帖子链接".to_string())?;
        let expires_text = value("XIAOMAN_CLAIM_EXPIRES").ok_or_else(|| {
            "开启自助领取需要 XIAOMAN_CLAIM_EXPIRES（如 2026-11-12 21:00）".to_string()
        })?;
        let expires_at = Self::parse_time(&expires_text, offset_minutes)
            .ok_or_else(|| "XIAOMAN_CLAIM_EXPIRES 格式：2026-11-12 21:00".to_string())?;
        let total = long("XIAOMAN_CLAIM_TOTAL_TOKENS", 1_000_000)?;
        let forum = value("XIAOMAN_CLAIM_FORUM").unwrap_or_else(|| Self::DEFAULT_FORUM.to_string());
        let forum = forum.trim_end_matches('/').to_string();
        if !forum.starts_with("https://") && !forum.starts_with("http://") {
            return Err("XIAOMAN_CLAIM_FORUM 无效".to_string());
        }
        let created_before = match value("XIAOMAN_CLAIM_MIN_CREATED_BEFORE") {
            Some(text) => Some(Self::parse_time(&text, offset_minutes).ok_or_else(|| {
                "XIAOMAN_CLAIM_MIN_CREATED_BEFORE 格式：2026-11-01 或 2026-11-01 12:00".to_string()
            })?),
            None => None,
        };
        let min_trust = match value("XIAOMAN_CLAIM_MIN_TRUST") {
            Some(text) => text
                .parse::<i32>()
                .ok()
                .filter(|trust| (0..=4).contains(trust))
                .ok_or_else(|| "XIAOMAN_CLAIM_MIN_TRUST 需要 0-4".to_string())?,
            None => 0,
        };
        let max = long("XIAOMAN_CLAIM_MAX", 50)? as usize;
        let daily = long("XIAOMAN_CLAIM_DAILY_TOKENS", total)?;
        let config = ClaimConfig {
            max,
            daily_tokens: daily,
            ..ClaimConfig::new(topic_id, expires_at)
        };
        let config = ClaimConfig {
            total_tokens: total,
            forum_url: forum,
            created_before,
            min_trust,
            ..config
        };
        Ok((
            Some(config),
            if now >= expires_at {
                "已过期"
            } else {
                "已开启"
            }
            .to_string(),
        ))
    }

    /// `465300`, or a topic link such as `https://forum.trae.cn/t/some-slug/465300/12`.
    pub fn parse_topic(text: &str) -> Option<i64> {
        let text = text.trim();
        if let Some(id) = text.parse::<i64>().ok().filter(|id| *id > 0) {
            return Some(id);
        }
        let parsed = url::Url::parse(text).ok()?;
        let segments: Vec<&str> = parsed.path_segments()?.collect();
        let start = segments.iter().position(|segment| *segment == "t")? + 1;
        segments[start..]
            .iter()
            .take(2)
            .find_map(|segment| segment.parse::<i64>().ok().filter(|id| *id > 0))
    }

    /// `2026-11-12 21:00`, `2026-11-12T21:00:00+08:00` or a bare date (its start) in the zone.
    pub fn parse_time(text: &str, offset_minutes: i32) -> Option<i64> {
        parse_moment(text, offset_minutes).or_else(|| parse_day_start(text, offset_minutes))
    }
}

/// The scheme and host check shared by the config and the client.
fn forum_base(url: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(url.trim().trim_end_matches('/'))
        .map_err(|_| "论坛地址无效".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("论坛地址无效".to_string());
    }
    Ok(parsed)
}

/// One forum account, as `/u/{name}.json` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub created_at: Option<i64>,
    pub trust_level: i32,
    pub suspended: bool,
}

/// One post of the topic, as `/t/{id}/posts.json` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub raw: String,
    pub cooked: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub visible: bool,
}

/// Why a forum read failed. "Not found" is not an error: callers see `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForumError {
    Unavailable(String),
    TopicMissing,
}

impl ForumError {
    pub fn message(&self) -> &str {
        match self {
            Self::Unavailable(message) => message,
            Self::TopicMissing => "领取帖不存在或不公开",
        }
    }
}

/// Read-only client for the public JSON of a Discourse forum.
pub struct ForumClient {
    base: url::Url,
    http: reqwest::Client,
}

impl ForumClient {
    pub fn new(base_url: &str, timeout_seconds: u64) -> Result<Self, String> {
        let base = forum_base(base_url)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_seconds.max(1)))
            .build()
            .map_err(|error| format!("论坛客户端初始化失败（{error}）"))?;
        Ok(Self { base, http })
    }

    fn url(&self, segments: &[&str]) -> Result<url::Url, ForumError> {
        let mut url = self.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| ForumError::Unavailable("论坛地址无效".to_string()))?;
            for segment in segments {
                path.push(segment);
            }
        }
        Ok(url)
    }

    /// The account, or `None` when the forum has no such username (Discourse matches it case-insensitively).
    pub async fn user(&self, username: &str) -> Result<Option<User>, ForumError> {
        let url = self.url(&["u", &format!("{username}.json")])?;
        let Some(root) = self.get(url).await? else {
            return Ok(None);
        };
        let user = root
            .get("user")
            .and_then(Value::as_object)
            .ok_or_else(|| ForumError::Unavailable("论坛返回的用户信息格式不对".to_string()))?;
        let id = user
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| ForumError::Unavailable("论坛返回的用户信息格式不对".to_string()))?;
        let suspended_till = instant(user.get("suspended_till").and_then(Value::as_str));
        Ok(Some(User {
            id,
            username: user
                .get("username")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or(username)
                .to_string(),
            created_at: instant(user.get("created_at").and_then(Value::as_str)),
            trust_level: user.get("trust_level").and_then(Value::as_i64).unwrap_or(0) as i32,
            suspended: suspended_till.is_some_and(|till| till > xiaoman_api::now_millis()),
        }))
    }

    /// Ids of every post in the topic, oldest first.
    pub async fn stream(&self, topic_id: i64) -> Result<Vec<i64>, ForumError> {
        let mut url = self.url(&["t", &format!("{topic_id}.json")])?;
        // Discourse may serve anonymous GETs from a short-lived cache; a changing query keeps new replies visible.
        url.query_pairs_mut()
            .append_pair("_", &(xiaoman_api::now_millis() / 1000).to_string());
        let Some(root) = self.get(url).await? else {
            return Err(ForumError::TopicMissing);
        };
        let stream = root
            .get("post_stream")
            .and_then(|stream| stream.get("stream"))
            .and_then(Value::as_array)
            .ok_or_else(|| ForumError::Unavailable("论坛返回的帖子格式不对".to_string()))?;
        Ok(stream.iter().filter_map(Value::as_i64).collect())
    }

    pub async fn posts(&self, topic_id: i64, ids: &[i64]) -> Result<Vec<Post>, ForumError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut url = self.url(&["t", &topic_id.to_string(), "posts.json"])?;
        {
            let mut query = url.query_pairs_mut();
            for id in ids {
                query.append_pair("post_ids[]", &id.to_string());
            }
            query.append_pair("include_raw", "true");
        }
        let Some(root) = self.get(url).await? else {
            return Err(ForumError::TopicMissing);
        };
        let posts = root
            .get("post_stream")
            .and_then(|stream| stream.get("posts"))
            .and_then(Value::as_array)
            .ok_or_else(|| ForumError::Unavailable("论坛返回的帖子格式不对".to_string()))?;
        Ok(posts.iter().filter_map(post_of).collect())
    }

    /// Parsed JSON, `None` for 404; anything else unexpected is [`ForumError::Unavailable`].
    async fn get(&self, url: url::Url) -> Result<Option<Value>, ForumError> {
        let response = self
            .http
            .get(url)
            .header("Accept", "application/json")
            .header("User-Agent", "xiaoman-claim/1.0")
            .send()
            .await
            .map_err(|error| {
                ForumError::Unavailable(format!("连不上论坛（{}）", transport_kind(&error)))
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(ForumError::Unavailable("论坛繁忙（429）".to_string()));
        }
        if !status.is_success() {
            return Err(ForumError::Unavailable(format!(
                "论坛返回 {}",
                status.as_u16()
            )));
        }
        let text = response.text().await.map_err(|error| {
            ForumError::Unavailable(format!("连不上论坛（{}）", transport_kind(&error)))
        })?;
        match serde_json::from_str::<Value>(&text) {
            Ok(value @ Value::Object(_)) => Ok(Some(value)),
            _ => Err(ForumError::Unavailable("论坛返回的不是 JSON".to_string())),
        }
    }
}

fn transport_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "超时"
    } else if error.is_connect() {
        "连接失败"
    } else {
        "网络错误"
    }
}

fn instant(text: Option<&str>) -> Option<i64> {
    let text = text.map(str::trim).filter(|text| !text.is_empty())?;
    parse_moment(text, 0)
}

fn post_of(item: &Value) -> Option<Post> {
    let post = item.as_object()?;
    let id = post.get("id").and_then(Value::as_i64)?;
    let created_at = instant(post.get("created_at").and_then(Value::as_str)).unwrap_or(0);
    let deleted = post
        .get("deleted_at")
        .map(|value| !value.is_null())
        .unwrap_or(false);
    let regular = post.get("post_type").and_then(Value::as_i64).unwrap_or(1) == 1;
    let hidden = post.get("hidden").and_then(Value::as_bool).unwrap_or(false);
    let user_deleted = post
        .get("user_deleted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(Post {
        id,
        user_id: post.get("user_id").and_then(Value::as_i64).unwrap_or(-1),
        username: post
            .get("username")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        raw: post
            .get("raw")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        cooked: post
            .get("cooked")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        created_at,
        updated_at: instant(post.get("updated_at").and_then(Value::as_str)).unwrap_or(created_at),
        // Regular, not hidden, not deleted (post_type 1 is a normal reply; 2 moderator notes, 3 small actions).
        visible: regular && !hidden && !deleted && !user_deleted,
    })
}

/// Discourse usernames: letters (any script), digits, `_`, `.`, `-`; 1-60 characters.
pub fn clean_username(input: &str) -> Option<String> {
    let trimmed = input
        .trim()
        .strip_prefix('@')
        .unwrap_or(input.trim())
        .trim();
    let length = trimmed.chars().count();
    let allowed = trimmed
        .chars()
        .all(|character| character.is_alphanumeric() || matches!(character, '_' | '.' | '-'));
    (allowed && (1..=60).contains(&length)).then(|| trimmed.to_string())
}

/// The post's own words (quotes of other posts removed): the raw text and the HTML with entities resolved.
pub fn texts(post: &Post) -> [String; 2] {
    [
        strip_quote_blocks(&post.raw),
        unescape(&strip_tags(&strip_quote_asides(&post.cooked))),
    ]
}

/// Whitespace, width and case differences are ignored, so a phrase typed on a phone still matches.
pub fn normalize(text: &str) -> String {
    let folded: String = text
        .chars()
        .filter_map(|character| match character {
            // NFKC's most common effect for phone keyboards: full-width ASCII becomes ASCII.
            '\u{3000}' => None,
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(character as u32 - 0xff01 + 0x21),
            other => Some(other),
        })
        .filter(|character| !character.is_whitespace())
        .collect();
    folded.to_uppercase()
}

pub fn contains(post: &Post, phrase: &str) -> bool {
    let phrase = normalize(phrase);
    !phrase.is_empty()
        && texts(post)
            .iter()
            .any(|text| normalize(text).contains(&phrase))
}

fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .to_lowercase()
        .find(&needle.to_lowercase())
        .map(|index| from + index)
}

/// Removes `[quote…]…[/quote]` blocks, which are other people's words.
fn strip_quote_blocks(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut index = 0;
    while let Some(start) = find_ci(raw, "[quote", index) {
        let Some(open) = raw[start..].find(']').map(|offset| start + offset) else {
            break;
        };
        match find_ci(raw, "[/quote]", open + 1) {
            Some(close) => {
                out.push_str(&raw[index..start]);
                out.push(' ');
                index = close + "[/quote]".len();
            }
            None => break,
        }
    }
    out.push_str(&raw[index.min(raw.len())..]);
    out
}

/// Removes `<aside class="…quote…">…</aside>` blocks from the rendered HTML.
fn strip_quote_asides(cooked: &str) -> String {
    let mut out = String::with_capacity(cooked.len());
    let mut index = 0;
    while let Some(start) = find_ci(cooked, "<aside", index) {
        let Some(open) = cooked[start..].find('>').map(|offset| start + offset) else {
            break;
        };
        let tag = &cooked[start..open];
        if !tag.to_lowercase().contains("quote") {
            out.push_str(&cooked[index..open + 1]);
            index = open + 1;
            continue;
        }
        let Some(close) = find_ci(cooked, "</aside>", open + 1) else {
            break;
        };
        out.push_str(&cooked[index..start]);
        out.push(' ');
        index = close + "</aside>".len();
    }
    out.push_str(&cooked[index.min(cooked.len())..]);
    out
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut index = 0;
    while let Some(start) = html[index..].find('<').map(|offset| index + offset) {
        out.push_str(&html[index..start]);
        match html[start..].find('>') {
            Some(offset) => index = start + offset + 1,
            None => return out,
        }
    }
    out.push_str(&html[index.min(html.len())..]);
    out
}

fn unescape(html: &str) -> String {
    html.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Sliding one-minute windows per key.
pub struct RateWindows {
    per_minute: usize,
    clock: Clock,
    windows: Mutex<HashMap<String, Vec<i64>>>,
}

impl RateWindows {
    pub fn new(per_minute: usize, clock: Clock) -> Self {
        Self {
            per_minute: per_minute.max(1),
            clock,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Records a hit, or returns the seconds to wait when `key` is over the limit.
    pub fn hit(&self, key: &str) -> Option<i64> {
        let now = (self.clock)();
        let mut windows = self.lock();
        if windows.len() > 10_000 {
            windows.retain(|_, window| {
                window.retain(|at| now - *at < 60_000);
                !window.is_empty()
            });
        }
        let window = windows.entry(key.to_string()).or_default();
        window.retain(|at| now - *at < 60_000);
        if window.len() >= self.per_minute {
            let first = window.first().copied().unwrap_or(now);
            return Some((((first + 60_000 - now) + 999) / 1000).max(1));
        }
        window.push(now);
        None
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Vec<i64>>> {
        self.windows
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

/// One answer of the claim routes: status, JSON body and the `Retry-After` header.
#[derive(Debug, Clone)]
pub struct ClaimReply {
    pub status: u16,
    pub body: Value,
    pub retry_after: Option<i64>,
}

#[derive(Debug, Clone)]
struct Challenge {
    id: String,
    user_id: i64,
    username: String,
    phrase: String,
    created_at: i64,
    expires_at: i64,
}

/// Tokens a forum account still holds; a claim reset by the instructor no longer counts.
pub fn active_claims(tokens: &[ClassToken]) -> Vec<ClassToken> {
    tokens
        .iter()
        .filter(|token| token.claim.as_ref().is_some_and(|claim| !claim.reset))
        .cloned()
        .collect()
}

/// The claim flow behind `/claim`.
pub struct ClaimService {
    config: ClaimConfig,
    store: Arc<ClassTokenStore>,
    offset_minutes: i32,
    clock: Clock,
    logger: Logger,
    forum: ForumClient,
    model: String,
    challenges: Mutex<Vec<Challenge>>,
    ip_starts: RateWindows,
    ip_checks: RateWindows,
    all_starts: RateWindows,
    all_checks: RateWindows,
    forum_slots: Semaphore,
    stream_cache: Mutex<Option<(i64, Vec<i64>)>>,
    post_cache: Mutex<HashMap<i64, (i64, Post)>>,
}

impl ClaimService {
    pub fn new(
        config: ClaimConfig,
        store: Arc<ClassTokenStore>,
        offset_minutes: i32,
        clock: Clock,
        logger: Logger,
        model: String,
    ) -> Result<Self, String> {
        config.validate()?;
        let forum = ForumClient::new(&config.forum_url, config.forum_timeout_seconds)?;
        let windows = |per_minute: usize| RateWindows::new(per_minute, clock.clone());
        Ok(Self {
            ip_starts: windows(config.per_ip_starts_per_minute),
            ip_checks: windows(config.per_ip_checks_per_minute),
            all_starts: windows(config.global_starts_per_minute),
            all_checks: windows(config.global_checks_per_minute),
            forum_slots: Semaphore::new(config.forum_concurrency),
            forum,
            config,
            store,
            offset_minutes,
            clock,
            logger,
            model,
            challenges: Mutex::new(Vec::new()),
            stream_cache: Mutex::new(None),
            post_cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn config(&self) -> &ClaimConfig {
        &self.config
    }

    /// `open` / `full` / `expired`, as `/api/info` and the claim page show it.
    pub fn state(&self) -> &'static str {
        let now = (self.clock)();
        if now >= self.config.expires_at {
            return "expired";
        }
        if active_claims(&self.store.all()).len() >= self.config.max {
            return "full";
        }
        "open"
    }

    pub fn info(&self) -> Value {
        let claimed = active_claims(&self.store.all()).len();
        json!({
            "state": self.state(),
            "topicUrl": self.config.topic_url(),
            "expiresAt": self.display(self.config.expires_at),
            "remaining": self.config.max.saturating_sub(claimed),
            "max": self.config.max,
            "totalTokens": self.config.total_tokens,
            "dailyTokens": self.config.daily_tokens,
            "model": self.model,
        })
    }

    /// Checks the account and hands out a one-time phrase.
    pub async fn start(&self, ip: &str, body: &Value) -> ClaimReply {
        if let Some(reply) = self.closed() {
            return reply;
        }
        let Some(username) = clean_username(string(body, "username").as_str()) else {
            return self.fail(
                400,
                "invalid_username",
                "论坛用户名格式不对：只能是字母、汉字、数字和 _ . -，不带空格（在论坛点头像→个人资料可以看到）",
            );
        };
        if let Some(wait) = self
            .ip_starts
            .hit(ip)
            .or_else(|| self.all_starts.hit("all"))
        {
            return self.limited(wait);
        }
        let user = match self.forum_user(&username).await {
            Ok(Some(user)) => user,
            Ok(None) => {
                return self.fail(
                    404,
                    "user_not_found",
                    format!("论坛上找不到用户「{username}」。请填用户名（个人主页地址 /u/ 后面那段），不是昵称"),
                );
            }
            Err(error) => return self.forum_down(&error),
        };
        if let Some(reply) = self.eligibility(&user) {
            return reply;
        }
        if let Some(reply) = self.existing(user.id) {
            return reply;
        }
        let now = (self.clock)();
        let challenge = {
            let mut challenges = self.challenges();
            challenges.retain(|challenge| now < challenge.expires_at);
            while challenges.len() >= self.config.max_pending {
                challenges.remove(0);
            }
            let mut id_bytes = [0u8; 18];
            random::fill(&mut id_bytes);
            let mut code_bytes = [0u8; 6];
            random::fill(&mut code_bytes);
            let code: String = code_bytes
                .iter()
                .map(|byte| CODE.as_bytes()[*byte as usize % CODE.len()] as char)
                .collect();
            let challenge = Challenge {
                id: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(id_bytes),
                user_id: user.id,
                username: user.username.clone(),
                phrase: format!("{PHRASE} {code}"),
                created_at: now,
                expires_at: now + self.config.challenge_millis,
            };
            challenges.push(challenge.clone());
            challenge
        };
        ClaimReply {
            status: 200,
            body: json!({
                "ok": true,
                "challengeId": challenge.id,
                "phrase": challenge.phrase,
                "username": challenge.username,
                "topicUrl": self.config.topic_url(),
                "expiresInSeconds": self.config.challenge_millis / 1000,
            }),
            retry_after: None,
        }
    }

    /// Looks for the account's reply containing the phrase and writes the token under the file lock.
    pub async fn verify(&self, ip: &str, body: &Value) -> ClaimReply {
        if let Some(reply) = self.closed() {
            return reply;
        }
        let challenge_id = string(body, "challengeId");
        let challenge = {
            let now = (self.clock)();
            let mut challenges = self.challenges();
            challenges.retain(|challenge| now < challenge.expires_at);
            challenges
                .iter()
                .find(|challenge| challenge.id == challenge_id)
                .cloned()
        };
        let Some(challenge) = challenge else {
            return self.fail(
                410,
                "challenge_expired",
                "验证短语已过期（10 分钟有效）或无效，请重新填写用户名获取新的短语",
            );
        };
        if let Some(wait) = self
            .ip_checks
            .hit(ip)
            .or_else(|| self.all_checks.hit("all"))
        {
            return self.limited(wait);
        }
        if let Some(reply) = self.existing(challenge.user_id) {
            return reply;
        }
        let found = match self.find_reply(&challenge).await {
            Ok(found) => found,
            Err(ForumError::TopicMissing) => {
                return self.fail(
                    503,
                    "topic_missing",
                    "领取帖打不开（可能已删除或设为不公开），请联系讲师",
                );
            }
            Err(error) => return self.forum_down(&error),
        };
        let Some(found) = found else {
            return self.fail(
                404,
                "reply_not_found",
                format!(
                    "还没在领取帖里找到「{}」发的这句话。刚回复的话等几秒再点；如果论坛提示回复需要审核，审核通过后再点",
                    challenge.username
                ),
            );
        };
        if found.user_id != challenge.user_id {
            return self.fail(
                403,
                "reply_wrong_user",
                format!(
                    "找到了这句话，但发帖账号是「{}」，不是你填的「{}」。请用自己的账号回复，或者返回重新填写用户名",
                    found.username, challenge.username
                ),
            );
        }
        // The account's own reply is there: issue the token under the file lock, where the
        // one-per-account and total limits are checked again.
        let now = (self.clock)();
        let mut outcome: Option<ClaimReply> = None;
        let mut plain: Option<String> = None;
        let written = self.store.update(|tokens| {
            let active = active_claims(&tokens);
            if let Some(mine) = active.iter().find(|token| {
                token
                    .claim
                    .as_ref()
                    .is_some_and(|claim| claim.user_id == challenge.user_id)
            }) {
                outcome = Some(self.already_claimed(mine));
                return Ok(tokens);
            }
            if active.len() >= self.config.max {
                outcome = Some(self.fail(
                    409,
                    "claim_full",
                    format!("这期的 {} 份令牌已经领完了，请联系讲师", self.config.max),
                ));
                return Ok(tokens);
            }
            let known: Vec<String> = tokens.iter().map(|token| token.hash.clone()).collect();
            let token_plain = loop {
                let candidate = ClassTokenCodec::generate();
                if !known.contains(&ClassTokenCodec::hash(&candidate)) {
                    break candidate;
                }
            };
            let token = ClassToken {
                hash: ClassTokenCodec::hash(&token_plain),
                prefix: token_plain.chars().take(6).collect(),
                label: format!("{LABEL}{}", challenge.username),
                created_at: now,
                expires_at: Some(self.config.expires_at),
                revoked: false,
                daily_limit: Some(self.config.daily_tokens),
                total_limit: Some(self.config.total_tokens),
                used_total: 0,
                used_today: 0,
                usage_day: String::new(),
                requests: 0,
                last_used_at: None,
                claim: Some(ForumClaim {
                    user_id: challenge.user_id,
                    username: challenge.username.clone(),
                    claimed_at: now,
                    reset: false,
                }),
            };
            plain = Some(token_plain);
            let mut next = tokens;
            next.push(token);
            Ok(next)
        });
        self.challenges().retain(|stored| stored.id != challenge.id);
        if let Err(error) = written {
            (self.logger)(&format!("自助领取 写入令牌失败：{error}"));
            return self.fail(500, "server_error", "请求处理失败");
        }
        if let Some(reply) = outcome {
            return reply;
        }
        let token = plain.expect("a token is generated when nothing refused the claim");
        let prefix: String = token.chars().take(6).collect();
        (self.logger)(&format!(
            "自助领取 {LABEL}{}({prefix}) 已发放",
            challenge.username
        ));
        ClaimReply {
            status: 200,
            body: json!({
                "ok": true,
                "token": token,
                "prefix": prefix,
                "username": challenge.username,
                "expiresAt": self.display(self.config.expires_at),
                "totalTokens": self.config.total_tokens,
                "dailyTokens": self.config.daily_tokens,
                "model": self.model,
            }),
            retry_after: None,
        }
    }

    /// The newest matching reply by anyone, preferring the claimant's own; `None` when none is in range.
    async fn find_reply(&self, challenge: &Challenge) -> Result<Option<Post>, ForumError> {
        let ids = self.cached_stream().await?;
        let recent: Vec<i64> = ids
            .iter()
            .skip(ids.len().saturating_sub(self.config.scan_posts))
            .rev()
            .copied()
            .collect();
        // Posts written well before the phrase existed cannot contain it.
        let oldest = challenge.created_at - 5 * 60_000;
        let mut other: Option<Post> = None;
        for batch in recent.chunks(BATCH) {
            let posts = self.cached_posts(batch).await?;
            let mut sorted = posts.clone();
            sorted.sort_by_key(|post| std::cmp::Reverse(post.id));
            for post in sorted {
                if !post.visible || !contains(&post, &challenge.phrase) {
                    continue;
                }
                if post.user_id == challenge.user_id {
                    return Ok(Some(post));
                }
                if other.is_none() {
                    other = Some(post);
                }
            }
            if !posts.is_empty()
                && posts
                    .iter()
                    .all(|post| post.created_at.max(post.updated_at) < oldest)
            {
                break;
            }
        }
        Ok(other)
    }

    async fn cached_stream(&self) -> Result<Vec<i64>, ForumError> {
        let now = (self.clock)();
        {
            let cache = self
                .stream_cache
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some((at, ids)) = cache.as_ref() {
                if now - at < STREAM_CACHE_MILLIS {
                    return Ok(ids.clone());
                }
            }
        }
        let ids = self.forum.stream(self.config.topic_id).await?;
        *self
            .stream_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some((now, ids.clone()));
        Ok(ids)
    }

    async fn cached_posts(&self, ids: &[i64]) -> Result<Vec<Post>, ForumError> {
        let now = (self.clock)();
        let (hit, miss) = {
            let mut cache = self
                .post_cache
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            cache.retain(|_, (at, _)| now - *at < POST_CACHE_MILLIS);
            let mut hit = Vec::new();
            let mut miss = Vec::new();
            for id in ids {
                if cache.contains_key(id) {
                    hit.push(*id);
                } else {
                    miss.push(*id);
                }
            }
            (hit, miss)
        };
        let fetched = if miss.is_empty() {
            Vec::new()
        } else {
            self.forum.posts(self.config.topic_id, &miss).await?
        };
        let mut cache = self
            .post_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for post in &fetched {
            cache.insert(post.id, (now, post.clone()));
        }
        let mut posts: Vec<Post> = hit
            .iter()
            .filter_map(|id| cache.get(id).map(|(_, post)| post.clone()))
            .collect();
        posts.extend(fetched);
        Ok(posts)
    }

    async fn forum_user(&self, username: &str) -> Result<Option<User>, ForumError> {
        let _slot = self.slot().await?;
        self.forum.user(username).await
    }

    /// Waits for a forum slot; the Kotlin service gives up after the configured timeout.
    async fn slot(&self) -> Result<SemaphorePermit<'_>, ForumError> {
        match tokio::time::timeout(
            Duration::from_secs(self.config.forum_timeout_seconds),
            self.forum_slots.acquire(),
        )
        .await
        {
            Ok(Ok(permit)) => Ok(permit),
            _ => Err(ForumError::Unavailable("排队超时".to_string())),
        }
    }

    fn eligibility(&self, user: &User) -> Option<ClaimReply> {
        if user.suspended {
            return Some(self.fail(403, "account_suspended", "这个论坛账号目前被封禁，不能领取"));
        }
        if let Some(before) = self.config.created_before {
            if user.created_at.is_none_or(|created| created >= before) {
                let when = user
                    .created_at
                    .map_or_else(|| "未知时间".to_string(), |at| self.display(at));
                return Some(self.fail(
                    403,
                    "account_too_new",
                    format!(
                        "这个论坛账号注册于 {when}，本次只限 {} 之前注册的账号领取",
                        self.display(before)
                    ),
                ));
            }
        }
        if user.trust_level < self.config.min_trust {
            return Some(self.fail(
                403,
                "trust_too_low",
                format!(
                    "这个论坛账号的信任等级是 {}，本次需要 {} 级以上（多在论坛浏览、回复就会升级）",
                    user.trust_level, self.config.min_trust
                ),
            ));
        }
        None
    }

    fn existing(&self, user_id: i64) -> Option<ClaimReply> {
        active_claims(&self.store.all())
            .into_iter()
            .find(|token| {
                token
                    .claim
                    .as_ref()
                    .is_some_and(|claim| claim.user_id == user_id)
            })
            .map(|token| self.already_claimed(&token))
    }

    fn already_claimed(&self, token: &ClassToken) -> ClaimReply {
        let claim = token
            .claim
            .as_ref()
            .expect("only claimed tokens reach here");
        let state = match self.store.check(token).map(|denied| denied.code) {
            Some(code) if code == "token_revoked" => "，已被讲师作废，不会再补发；有问题请联系讲师",
            Some(code) if code == "token_expired" => "，已经过期",
            _ => "。令牌只在领取时显示一次，找不到了请联系讲师",
        };
        ClaimReply {
            status: 409,
            body: json!({
                "ok": false,
                "code": "already_claimed",
                "error": format!("论坛账号「{}」已经领过令牌（{}…）{state}", claim.username, token.prefix),
                "prefix": token.prefix,
                "revoked": token.revoked,
            }),
            retry_after: None,
        }
    }

    fn closed(&self) -> Option<ClaimReply> {
        match self.state() {
            "expired" => Some(self.fail(
                410,
                "claim_expired",
                format!(
                    "领取已结束（课堂令牌在 {} 过期）",
                    self.display(self.config.expires_at)
                ),
            )),
            "full" => Some(self.fail(
                409,
                "claim_full",
                format!("这期的 {} 份令牌已经领完了，请联系讲师", self.config.max),
            )),
            _ => None,
        }
    }

    fn limited(&self, wait: i64) -> ClaimReply {
        ClaimReply {
            retry_after: Some(wait),
            ..self.fail(
                429,
                "rate_limited",
                format!("操作太频繁，请 {wait} 秒后再试"),
            )
        }
    }

    fn forum_down(&self, error: &ForumError) -> ClaimReply {
        (self.logger)(&format!("自助领取 论坛请求失败：{}", error.message()));
        ClaimReply {
            retry_after: Some(10),
            ..self.fail(
                503,
                "forum_unavailable",
                "暂时连不上论坛，请稍后重试（这次没有发放令牌）",
            )
        }
    }

    fn fail(&self, status: u16, code: &str, message: impl Into<String>) -> ClaimReply {
        ClaimReply {
            status,
            body: json!({ "ok": false, "code": code, "error": message.into() }),
            retry_after: None,
        }
    }

    fn display(&self, millis: i64) -> String {
        display(millis, self.offset_minutes)
    }

    fn challenges(&self) -> MutexGuard<'_, Vec<Challenge>> {
        self.challenges
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

fn string(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(raw: &str, cooked: &str) -> Post {
        Post {
            id: 1,
            user_id: 101,
            username: "alice".to_string(),
            raw: raw.to_string(),
            cooked: cooked.to_string(),
            created_at: 0,
            updated_at: 0,
            visible: true,
        }
    }

    #[test]
    fn phrases_match_across_width_space_and_case() {
        let phrase = "领取小满令牌 ABC234";
        assert!(contains(
            &post(phrase, "<p>领取小满令牌 ABC234</p>"),
            phrase
        ));
        let typed = "领取 小满令牌\u{3000}ａｂｃ２３４";
        assert!(
            contains(&post(typed, "<p>x</p>"), phrase),
            "full-width and spaced input matches"
        );
    }

    #[test]
    fn quotes_of_other_posts_do_not_count() {
        let phrase = "领取小满令牌 ABC234";
        let raw = format!("[quote=\"bob, post:3\"]\n{phrase}\n[/quote]\n顶");
        let cooked = format!(
            "<aside class=\"quote no-group\"><blockquote><p>{phrase}</p></blockquote></aside><p>顶</p>"
        );
        assert!(!contains(&post(&raw, &cooked), phrase));
        assert!(contains(
            &post(
                &format!("顶\n{phrase}"),
                "<p>顶</p><p>领取小满令牌&nbsp;ABC234</p>"
            ),
            phrase
        ));
    }

    #[test]
    fn usernames_follow_the_discourse_rules() {
        assert_eq!(
            clean_username(" @u.name-1_x "),
            Some("u.name-1_x".to_string())
        );
        assert_eq!(clean_username("汤圆"), Some("汤圆".to_string()));
        for bad in ["", "a b", "../admin", "x/y", "名字?"] {
            assert!(clean_username(bad).is_none(), "{bad}");
        }
        assert!(clean_username(&"a".repeat(61)).is_none());
    }

    #[test]
    fn rate_windows_return_the_seconds_to_wait() {
        let now = Arc::new(std::sync::atomic::AtomicI64::new(0));
        let clock = {
            let now = now.clone();
            Arc::new(move || now.load(std::sync::atomic::Ordering::SeqCst))
        };
        let windows = RateWindows::new(2, clock);
        assert!(windows.hit("ip").is_none());
        assert!(windows.hit("ip").is_none());
        assert_eq!(windows.hit("ip"), Some(60));
        now.store(30_000, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(windows.hit("ip"), Some(30));
        now.store(60_000, std::sync::atomic::Ordering::SeqCst);
        assert!(windows.hit("ip").is_none(), "the window slides");
    }

    #[test]
    fn topic_ids_come_from_numbers_or_links() {
        assert_eq!(ClaimConfig::parse_topic("465300"), Some(465300));
        assert_eq!(
            ClaimConfig::parse_topic("https://forum.trae.cn/t/some-slug/465300/12"),
            Some(465300)
        );
        assert_eq!(ClaimConfig::parse_topic("abc"), None);
    }

    #[test]
    fn moments_accept_offsets_local_times_and_dates() {
        let expires = ClaimConfig::parse_time("2026-11-12 21:00", 480).expect("local time");
        assert_eq!(display(expires, 480), "2026-11-12 21:00");
        assert_eq!(
            ClaimConfig::parse_time("2026-11-12T21:00:00+08:00", 480),
            Some(expires)
        );
        let day_start = ClaimConfig::parse_time("2026-11-12", 480).expect("bare date");
        assert_eq!(display(day_start, 480), "2026-11-12 00:00");
        assert_eq!(ClaimConfig::parse_time("明天", 480), None);
    }
}
