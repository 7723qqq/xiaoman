//! Class tokens (`课堂令牌`): the record a token leaves on disk, the codec for the token file, and
//! the store the server and the `tokens` admin command share.
//!
//! Only the SHA-256 hash of a token is stored; the plaintext exists in the operator's hands and once
//! in a claim response. Limits count model tokens (prompt + completion) and `None` means unlimited.
//!
//! Writers take an advisory lock on `<file>.lock`, re-read the file, change it and atomically rename
//! a temporary file over it, so the server's usage updates and the admin's revocations never
//! overwrite each other. The server reloads a replaced file within `reload_millis` and keeps the
//! usage it has not written yet in memory on top of it.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, SecondsFormat};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::memory::Clock;

/// How long a writer waits for another process's lock before giving up.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
/// A lock file older than this was left behind by a process that died while writing.
const STALE_LOCK: Duration = Duration::from_secs(60);

/// What the token file says about itself; the admin command and the server both read it.
const NOTE: &str = "小满课堂令牌。只存 SHA-256 哈希；用 web-server tokens 管理，不要手改。";

/// The plaintext shape: `xm-` plus 20 characters, 100 random bits.
pub const PREFIX: &str = "xm-";
/// No 0/1/l/o, so tokens read aloud or copied from a slide are unambiguous.
const ALPHABET: &[u8] = b"23456789abcdefghijkmnpqrstuvwxyz";
const TOKEN_LENGTH: usize = 20;

/// Which forum account claimed a token. `reset` marks a claim the instructor released with
/// `tokens reset-claim`: it no longer blocks that account from claiming again, and its token was
/// revoked at the same time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForumClaim {
    pub user_id: i64,
    pub username: String,
    pub claimed_at: i64,
    pub reset: bool,
}

/// One class token as stored on disk. Limits are in model tokens; `None` means unlimited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassToken {
    pub hash: String,
    pub prefix: String,
    pub label: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked: bool,
    pub daily_limit: Option<i64>,
    pub total_limit: Option<i64>,
    pub used_total: i64,
    pub used_today: i64,
    pub usage_day: String,
    pub requests: i64,
    pub last_used_at: Option<i64>,
    /// Set for tokens students claimed themselves with a forum account ([`crate::claims`]).
    pub claim: Option<ForumClaim>,
}

impl ClassToken {
    /// Usage on `day`; a token last used on another day counts zero today.
    pub fn used_on(&self, day: &str) -> i64 {
        if self.usage_day == day {
            self.used_today
        } else {
            0
        }
    }

    /// `label(prefix)`, as the logs and the admin list show it.
    pub fn tag(&self) -> String {
        format!("{}({})", self.label, self.prefix)
    }
}

/// Why a token may not be used right now, as an OpenAI-style error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassDenied {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub retry_after_seconds: Option<i64>,
}

impl ClassDenied {
    pub fn new(
        status: u16,
        code: impl Into<String>,
        message: impl Into<String>,
        retry_after_seconds: Option<i64>,
    ) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            retry_after_seconds,
        }
    }
}

/// A zone from the offset the server is configured with (`Asia/Shanghai` is +480).
pub fn offset(offset_minutes: i32) -> FixedOffset {
    FixedOffset::east_opt(offset_minutes * 60).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap())
}

/// The calendar day of `now` in the display zone, `YYYY-MM-DD`.
pub fn day_of(now: i64, offset_minutes: i32) -> String {
    local(now, offset_minutes).format("%Y-%m-%d").to_string()
}

/// `YYYY-MM-DD HH:MM`, as the pages and the error messages show a moment.
pub fn display(now: i64, offset_minutes: i32) -> String {
    local(now, offset_minutes)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn local(now: i64, offset_minutes: i32) -> DateTime<FixedOffset> {
    DateTime::from_timestamp_millis(now)
        .unwrap_or_default()
        .with_timezone(&offset(offset_minutes))
}

/// An ISO-8601 instant with offset, the form the token file stores.
fn instant(now: i64, offset_minutes: i32) -> String {
    local(now, offset_minutes).to_rfc3339_opts(SecondsFormat::Secs, false)
}

/// `2026-11-12T21:00:00+08:00`, `2026-11-12T21:00:00Z`, or a local `2026-11-12 21:00` in the zone.
pub fn parse_moment(text: &str, offset_minutes: i32) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Some(parsed.timestamp_millis());
    }
    let local = text.replace(' ', "T");
    let local = if local.len() == 16 {
        format!("{local}:00")
    } else {
        local
    };
    let naive = NaiveDateTime::parse_from_str(&local, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(&local, "%Y-%m-%dT%H:%M"))
        .ok()?;
    naive
        .and_local_timezone(offset(offset_minutes))
        .single()
        .map(|at| at.timestamp_millis())
}

/// A bare date (`2026-11-01`) is its start in the zone; used by the claim filters.
pub fn parse_day_start(text: &str, offset_minutes: i32) -> Option<i64> {
    let text = text.trim();
    let date = NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
    let midnight = date.and_hms_opt(0, 0, 0)?;
    midnight
        .and_local_timezone(offset(offset_minutes))
        .single()
        .map(|at| at.timestamp_millis())
}

/// Zones the admin command and the environment accept by name; anything else must be an offset.
const ZONES: &[(&str, i32)] = &[
    ("Asia/Shanghai", 480),
    ("Asia/Chongqing", 480),
    ("Asia/Hong_Kong", 480),
    ("Asia/Macau", 480),
    ("Asia/Taipei", 480),
    ("Asia/Singapore", 480),
    ("Asia/Tokyo", 540),
    ("Asia/Seoul", 540),
    ("Asia/Kolkata", 330),
    ("Asia/Dubai", 240),
    ("UTC", 0),
    ("Etc/UTC", 0),
    ("GMT", 0),
    ("Europe/London", 0),
    ("America/New_York", -300),
    ("America/Los_Angeles", -480),
    ("Australia/Sydney", 600),
];

/// `Asia/Shanghai`, `UTC` or `+08:00` → offset from UTC in minutes.
pub fn zone_offset(name: &str) -> Option<i32> {
    let name = name.trim();
    if let Some((_, offset)) = ZONES.iter().find(|(zone, _)| *zone == name) {
        return Some(*offset);
    }
    let (sign, digits) = match name.as_bytes().first()? {
        b'+' => (1, &name[1..]),
        b'-' => (-1, &name[1..]),
        _ => return None,
    };
    let (hours, minutes) = match digits.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if digits.len() == 4 => (&digits[..2], &digits[2..]),
        None => (digits, "0"),
    };
    let hours: i32 = hours.parse().ok()?;
    let minutes: i32 = minutes.parse().ok()?;
    ((0..=14).contains(&hours) && (0..60).contains(&minutes))
        .then_some(sign * (hours * 60 + minutes))
}

/// Generation, hashing and the on-disk format of the token file.
pub struct ClassTokenCodec;

impl ClassTokenCodec {
    /// `xm-` plus 20 characters from the 32-letter alphabet: 100 random bits.
    pub fn generate() -> String {
        let mut raw = [0u8; TOKEN_LENGTH];
        crate::random::fill(&mut raw);
        let body: String = raw
            .iter()
            .map(|byte| ALPHABET[*byte as usize % ALPHABET.len()] as char)
            .collect();
        format!("{PREFIX}{body}")
    }

    /// Lowercase hex SHA-256 of the trimmed plaintext.
    pub fn hash(token: &str) -> String {
        let digest = Sha256::digest(token.trim().as_bytes());
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    pub fn looks_like_token(value: &str) -> bool {
        value.chars().count() == PREFIX.len() + TOKEN_LENGTH && value.starts_with(PREFIX)
    }

    /// The file contents: versioned JSON with a short note for whoever opens it by hand.
    pub fn encode(tokens: &[ClassToken], offset_minutes: i32) -> String {
        let time = |value: Option<i64>| match value {
            Some(millis) => Value::String(instant(millis, offset_minutes)),
            None => Value::Null,
        };
        let items: Vec<Value> = tokens
            .iter()
            .map(|token| {
                let mut item = Map::new();
                item.insert("label".to_string(), json!(token.label));
                item.insert("prefix".to_string(), json!(token.prefix));
                item.insert("sha256".to_string(), json!(token.hash));
                item.insert("createdAt".to_string(), time(Some(token.created_at)));
                item.insert("expiresAt".to_string(), time(token.expires_at));
                item.insert("revoked".to_string(), json!(token.revoked));
                item.insert("dailyLimit".to_string(), limit(token.daily_limit));
                item.insert("totalLimit".to_string(), limit(token.total_limit));
                item.insert("usedToday".to_string(), json!(token.used_today));
                item.insert("usageDay".to_string(), json!(token.usage_day));
                item.insert("usedTotal".to_string(), json!(token.used_total));
                item.insert("requests".to_string(), json!(token.requests));
                item.insert("lastUsedAt".to_string(), time(token.last_used_at));
                if let Some(claim) = &token.claim {
                    item.insert(
                        "claim".to_string(),
                        json!({
                            "forumUserId": claim.user_id,
                            "forumUsername": claim.username,
                            "claimedAt": time(Some(claim.claimed_at)),
                            "reset": claim.reset,
                        }),
                    );
                }
                Value::Object(item)
            })
            .collect();
        let mut root = Map::new();
        root.insert("version".to_string(), json!(1));
        root.insert("note".to_string(), json!(NOTE));
        root.insert("tokens".to_string(), Value::Array(items));
        let mut text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
        text.push('\n');
        text
    }

    /// Reads the file; a hash that is not 64 hex digits or a claim without a forum id is an error.
    pub fn decode(text: &str) -> Result<Vec<ClassToken>, String> {
        let root: Value = serde_json::from_str(text)
            .map_err(|error| format!("令牌文件不是合法 JSON（{error}）"))?;
        let items = root
            .get("tokens")
            .and_then(Value::as_array)
            .ok_or_else(|| "令牌文件里没有 tokens".to_string())?;
        let mut tokens = Vec::with_capacity(items.len());
        for item in items {
            let item = item
                .as_object()
                .ok_or_else(|| "令牌文件里有无效的令牌记录".to_string())?;
            let hash = item
                .get("sha256")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err("令牌文件里有无效的哈希".to_string());
            }
            let claim = match item.get("claim") {
                Some(Value::Object(claim)) => {
                    let user_id = claim
                        .get("forumUserId")
                        .and_then(Value::as_i64)
                        .ok_or_else(|| "令牌文件里有无效的领取记录".to_string())?;
                    Some(ForumClaim {
                        user_id,
                        username: claim
                            .get("forumUsername")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        claimed_at: time(claim, "claimedAt")?.unwrap_or(0),
                        reset: claim.get("reset").and_then(Value::as_bool).unwrap_or(false),
                    })
                }
                _ => None,
            };
            tokens.push(ClassToken {
                hash: hash.to_string(),
                prefix: item
                    .get("prefix")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                label: item
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                created_at: time(item, "createdAt")?.unwrap_or(0),
                expires_at: time(item, "expiresAt")?,
                revoked: item
                    .get("revoked")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                daily_limit: number(item, "dailyLimit"),
                total_limit: number(item, "totalLimit"),
                used_total: number(item, "usedTotal").unwrap_or(0),
                used_today: number(item, "usedToday").unwrap_or(0),
                usage_day: item
                    .get("usageDay")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                requests: number(item, "requests").unwrap_or(0),
                last_used_at: time(item, "lastUsedAt")?,
                claim,
            });
        }
        Ok(tokens)
    }
}

fn limit(value: Option<i64>) -> Value {
    value.map_or(Value::Null, |value| json!(value))
}

fn number(object: &Map<String, Value>, key: &str) -> Option<i64> {
    object.get(key).and_then(Value::as_i64)
}

/// Instants written by this codec carry their own offset; a hand-edited local time is read as UTC.
fn time(object: &Map<String, Value>, key: &str) -> Result<Option<i64>, String> {
    match object.get(key).and_then(Value::as_str).map(str::trim) {
        Some(text) if !text.is_empty() => parse_moment(text, 0)
            .map(Some)
            .ok_or_else(|| format!("令牌文件里有无效的时间（{key}）")),
        _ => Ok(None),
    }
}

/// One day's use of one token, waiting to be written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Delta {
    tokens: i64,
    requests: i64,
    last: i64,
}

impl Delta {
    fn absorb(&mut self, other: &Delta) {
        self.tokens += other.tokens;
        self.requests += other.requests;
        self.last = self.last.max(other.last);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    hash: String,
    day: String,
}

/// What the store remembers about the file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Default)]
struct Inner {
    /// Tokens in file order; a class has hundreds at most, so a scan is cheaper than an index.
    cached: Vec<ClassToken>,
    /// `None` until a readable file has been parsed.
    loaded: Option<Stamp>,
    checked_at: Option<Instant>,
    /// Usage recorded but not yet handed to [`ClassTokenStore::flush`].
    pending: HashMap<Key, Delta>,
    /// Usage handed to a write that has not finished.
    inflight: HashMap<Key, Delta>,
}

/// What went wrong while reading or writing the token file.
#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    /// The file on disk cannot be parsed.
    Invalid(String),
    /// The change itself refused to happen (a selector matched nothing, a claim is gone).
    Rejected(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "读写令牌文件失败（{error}）"),
            Self::Invalid(message) | Self::Rejected(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The token file shared by the running server (usage) and the admin command (everything else).
pub struct ClassTokenStore {
    path: PathBuf,
    offset_minutes: i32,
    clock: Clock,
    reload_millis: i64,
    inner: Mutex<Inner>,
}

/// Writers hold this before touching the file, so two threads never race for the file lock.
fn process_lock() -> &'static Mutex<()> {
    static PROCESS_LOCK: Mutex<()> = Mutex::new(());
    &PROCESS_LOCK
}

impl ClassTokenStore {
    pub fn new(path: PathBuf, offset_minutes: i32, clock: Clock, reload_millis: i64) -> Self {
        Self {
            path,
            offset_minutes,
            clock,
            reload_millis,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn offset_minutes(&self) -> i32 {
        self.offset_minutes
    }

    /// Today in the display zone.
    pub fn day(&self) -> String {
        self.day_at((self.clock)())
    }

    pub fn day_at(&self, now: i64) -> String {
        day_of(now, self.offset_minutes)
    }

    /// Whether the file exists; the class-token feature is off until it does.
    pub fn exists(&self) -> bool {
        let mut inner = self.lock();
        self.refresh(&mut inner);
        inner.loaded.is_some()
    }

    /// The token whose plaintext is `plain`, including usage not yet written to disk.
    pub fn find(&self, plain: &str) -> Option<ClassToken> {
        let hash = ClassTokenCodec::hash(plain);
        let mut inner = self.lock();
        self.refresh(&mut inner);
        let index = inner.cached.iter().position(|token| token.hash == hash)?;
        let token = inner.cached[index].clone();
        Some(self.with_pending(&inner, &token))
    }

    /// Every token in file order.
    pub fn all(&self) -> Vec<ClassToken> {
        let mut inner = self.lock();
        self.refresh(&mut inner);
        let cached = inner.cached.clone();
        cached
            .iter()
            .map(|token| self.with_pending(&inner, token))
            .collect()
    }

    pub fn check(&self, token: &ClassToken) -> Option<ClassDenied> {
        self.check_at(token, (self.clock)())
    }

    pub fn check_at(&self, token: &ClassToken, now: i64) -> Option<ClassDenied> {
        if token.revoked {
            return Some(ClassDenied::new(
                401,
                "token_revoked",
                "课堂令牌已作废",
                None,
            ));
        }
        if token.expires_at.is_some_and(|expires| now >= expires) {
            return Some(ClassDenied::new(
                401,
                "token_expired",
                "课堂令牌已过期",
                None,
            ));
        }
        if token
            .total_limit
            .is_some_and(|limit| token.used_total >= limit)
        {
            return Some(ClassDenied::new(
                429,
                "total_quota_exceeded",
                "课堂令牌的总额度已用完，请联系讲师",
                None,
            ));
        }
        if token
            .daily_limit
            .is_some_and(|limit| token.used_on(&self.day_at(now)) >= limit)
        {
            return Some(ClassDenied::new(
                429,
                "daily_quota_exceeded",
                "课堂令牌今天的额度已用完，明天再试或联系讲师",
                None,
            ));
        }
        None
    }

    /// Adds one finished request's model tokens; written to disk by [`Self::flush`].
    pub fn record(&self, hash: &str, tokens: i64) {
        let now = (self.clock)();
        let day = self.day_at(now);
        let mut inner = self.lock();
        let delta = inner
            .pending
            .entry(Key {
                hash: hash.to_string(),
                day,
            })
            .or_default();
        delta.tokens += tokens.max(0);
        delta.requests += 1;
        delta.last = now;
    }

    /// Writes pending usage. On failure the usage stays pending and the error is returned.
    pub fn flush(&self) -> Result<(), StoreError> {
        let batch: Vec<(Key, Delta)> = {
            let mut inner = self.lock();
            if inner.pending.is_empty() {
                return Ok(());
            }
            for (key, delta) in std::mem::take(&mut inner.pending) {
                inner.inflight.entry(key).or_default().absorb(&delta);
            }
            inner
                .inflight
                .iter()
                .map(|(key, delta)| (key.clone(), delta.clone()))
                .collect()
        };
        let written = self.update(move |mut tokens| {
            for token in tokens.iter_mut() {
                let mut relevant: Vec<&(Key, Delta)> = batch
                    .iter()
                    .filter(|(key, _)| key.hash == token.hash)
                    .collect();
                relevant.sort_by(|left, right| left.0.day.cmp(&right.0.day));
                for (key, delta) in relevant {
                    let today = if token.usage_day == key.day {
                        token.used_today + delta.tokens
                    } else if token.usage_day < key.day {
                        delta.tokens
                    } else {
                        token.used_today
                    };
                    token.used_total += delta.tokens;
                    token.used_today = today;
                    if key.day > token.usage_day {
                        token.usage_day.clone_from(&key.day);
                    }
                    token.requests += delta.requests;
                    token.last_used_at = Some(token.last_used_at.unwrap_or(0).max(delta.last));
                }
            }
            Ok(tokens)
        });
        match written {
            Ok(_) => {
                self.lock().inflight.clear();
                Ok(())
            }
            Err(error) => {
                let mut inner = self.lock();
                let inflight = std::mem::take(&mut inner.inflight);
                for (key, delta) in inflight {
                    inner.pending.entry(key).or_default().absorb(&delta);
                }
                Err(error)
            }
        }
    }

    /// Locked read-modify-write of the whole file; creates it when missing. Returns the new list.
    pub fn update<F>(&self, change: F) -> Result<Vec<ClassToken>, StoreError>
    where
        F: FnOnce(Vec<ClassToken>) -> Result<Vec<ClassToken>, StoreError>,
    {
        let _process = process_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let lock_path = lock_path(&self.path);
        let _lock = FileLock::acquire(&lock_path)?;
        let current = match fs::read_to_string(&self.path) {
            Ok(text) => ClassTokenCodec::decode(&text).map_err(StoreError::Invalid)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(StoreError::Io(error)),
        };
        let next = change(current)?;
        write_atomically(
            &self.path,
            &ClassTokenCodec::encode(&next, self.offset_minutes),
        )?;
        self.lock().checked_at = None;
        Ok(next)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn with_pending(&self, inner: &Inner, token: &ClassToken) -> ClassToken {
        let today = self.day();
        let mut any = false;
        let mut tokens = 0i64;
        let mut today_tokens = 0i64;
        let mut requests = 0i64;
        let mut last = 0i64;
        for (key, delta) in inner.inflight.iter().chain(inner.pending.iter()) {
            if key.hash != token.hash {
                continue;
            }
            any = true;
            tokens += delta.tokens;
            requests += delta.requests;
            last = last.max(delta.last);
            if key.day == today {
                today_tokens += delta.tokens;
            }
        }
        if !any {
            return token.clone();
        }
        ClassToken {
            used_total: token.used_total + tokens,
            used_today: token.used_on(&today) + today_tokens,
            usage_day: today,
            requests: token.requests + requests,
            last_used_at: Some(token.last_used_at.unwrap_or(0).max(last)),
            ..token.clone()
        }
    }

    /// Re-reads the file when it changed and the reload interval passed. A file that cannot be read
    /// or parsed keeps the tokens already loaded, so a half-written file never empties the class.
    fn refresh(&self, inner: &mut Inner) {
        let now = Instant::now();
        if let Some(checked) = inner.checked_at {
            if now.duration_since(checked) < Duration::from_millis(self.reload_millis.max(0) as u64)
            {
                return;
            }
        }
        inner.checked_at = Some(now);
        let Ok(metadata) = fs::metadata(&self.path) else {
            inner.cached.clear();
            inner.loaded = None;
            return;
        };
        let stamp = Stamp {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        };
        if inner.loaded.as_ref() == Some(&stamp) {
            return;
        }
        let Ok(text) = fs::read_to_string(&self.path) else {
            return;
        };
        let Ok(tokens) = ClassTokenCodec::decode(&text) else {
            return;
        };
        inner.cached = tokens;
        inner.loaded = Some(stamp);
    }
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".lock");
    path.with_file_name(name)
}

/// A lock file created with `create_new`, which is atomic on every platform. The critical section
/// lasts a few milliseconds; a file left behind by a killed process is removed once it is stale.
struct FileLock {
    path: PathBuf,
}

impl FileLock {
    fn acquire(path: &Path) -> Result<Self, StoreError> {
        let started = Instant::now();
        let mut recovered = false;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if !recovered && is_stale(path) {
                        let _ = fs::remove_file(path);
                        recovered = true;
                        continue;
                    }
                    if started.elapsed() > LOCK_TIMEOUT {
                        return Err(StoreError::Io(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("令牌文件被占用（{}）", path.display()),
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(StoreError::Io(error)),
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > STALE_LOCK)
}

/// Writes through a temporary file and renames it over the target, so a reader never sees half a file.
fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(format!(".tmp-{}", COUNTER.fetch_add(1, Ordering::SeqCst)));
    let temporary = path.with_file_name(name);
    let result = (|| {
        {
            let mut file = File::create(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        set_private(&temporary);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Only the service user and its group may read the token file.
#[cfg(unix)]
fn set_private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o640));
}

#[cfg(not(unix))]
fn set_private(_path: &Path) {}

/// A moment plus `days`, for the `--days` option of the admin command.
pub fn after_days(now: i64, days: f64) -> i64 {
    now + (days * 86_400_000.0) as i64
}

/// A moment plus `hours`, for the `--hours` option of the admin command.
pub fn after_hours(now: i64, hours: f64) -> i64 {
    now + (hours * 3_600_000.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &tempfile::TempDir, now: i64) -> ClassTokenStore {
        ClassTokenStore::new(
            dir.path().join("class-tokens.json"),
            480,
            std::sync::Arc::new(move || now),
            0,
        )
    }

    fn token(label: &str) -> (ClassToken, String) {
        let plain = ClassTokenCodec::generate();
        let token = ClassToken {
            hash: ClassTokenCodec::hash(&plain),
            prefix: plain[..6].to_string(),
            label: label.to_string(),
            created_at: 1_762_400_000_000,
            expires_at: None,
            revoked: false,
            daily_limit: None,
            total_limit: None,
            used_total: 0,
            used_today: 0,
            usage_day: String::new(),
            requests: 0,
            last_used_at: None,
            claim: None,
        };
        (token, plain)
    }

    #[test]
    fn generated_tokens_have_the_documented_shape() {
        let plain = ClassTokenCodec::generate();
        assert!(ClassTokenCodec::looks_like_token(&plain), "{plain}");
        assert_eq!(plain.len(), 23);
        assert!(plain[3..].chars().all(|c| ALPHABET.contains(&(c as u8))));
        assert!(
            !plain[3..]
                .chars()
                .any(|c| matches!(c, '0' | '1' | 'l' | 'o'))
        );
        assert_eq!(ClassTokenCodec::hash(" xm-abc ").len(), 64);
        assert_eq!(
            ClassTokenCodec::hash("xm-abc"),
            ClassTokenCodec::hash(" xm-abc\n")
        );
        assert!(!ClassTokenCodec::looks_like_token("xm-short"));
    }

    #[test]
    fn the_file_round_trips_through_the_codec() {
        let (mut first, _plain) = token("学员 01");
        first.daily_limit = Some(1000);
        first.total_limit = Some(5000);
        first.expires_at = Some(1_762_659_200_000);
        first.claim = Some(ForumClaim {
            user_id: 101,
            username: "Alice".to_string(),
            claimed_at: 1_762_400_000_000,
            reset: true,
        });
        let text = ClassTokenCodec::encode(&[first.clone()], 480);
        assert!(text.contains("\"version\": 1"));
        assert!(text.contains("\"sha256\""));
        assert!(text.contains("2025-11-06T11:33:20+08:00"), "{text}");
        let decoded = ClassTokenCodec::decode(&text).expect("valid file");
        assert_eq!(decoded, vec![first]);
        assert!(
            ClassTokenCodec::decode("{\"tokens\":[{\"sha256\":\"xyz\"}]}")
                .unwrap_err()
                .contains("无效的哈希")
        );
        assert!(
            ClassTokenCodec::decode(&format!(
                "{{\"tokens\":[{{\"sha256\":\"{}\",\"claim\":{{}}}}]}}",
                "a".repeat(64)
            ))
            .unwrap_err()
            .contains("无效的领取记录")
        );
    }

    #[test]
    fn a_missing_file_means_off() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir, 1_762_400_000_000);
        assert!(!store.exists());
        assert!(store.find("xm-anything").is_none());
        assert!(store.all().is_empty());
    }

    #[test]
    fn pending_usage_counts_and_survives_a_failed_flush() {
        let dir = tempfile::tempdir().unwrap();
        let now = 1_762_400_000_000;
        let store = store(&dir, now);
        let (token, plain) = token("学员 01");
        store.update(|_| Ok(vec![token.clone()])).unwrap();
        store.record(&token.hash, 40);
        store.record(&token.hash, 2);
        let found = store.find(&plain).unwrap();
        assert_eq!(found.used_total, 42);
        assert_eq!(found.requests, 2);
        store.flush().unwrap();
        let reread = ClassTokenStore::new(
            dir.path().join("class-tokens.json"),
            480,
            std::sync::Arc::new(move || now),
            0,
        )
        .find(&plain)
        .unwrap();
        assert_eq!(reread.used_total, 42);
        assert_eq!(
            store.find(&plain).unwrap().used_total,
            42,
            "no double counting after the flush"
        );
    }

    #[test]
    fn quota_and_expiry_are_reported_with_the_kotlin_messages() {
        let dir = tempfile::tempdir().unwrap();
        let now = 1_762_400_000_000;
        let store = store(&dir, now);
        let (mut token, _plain) = token("学员 01");
        token.daily_limit = Some(10);
        token.total_limit = Some(15);
        token.expires_at = Some(now + 60_000);
        assert!(store.check_at(&token, now).is_none());
        token.used_total = 10;
        token.used_today = 10;
        token.usage_day = store.day_at(now);
        assert_eq!(
            store.check_at(&token, now).unwrap().code,
            "daily_quota_exceeded"
        );
        token.used_total = 15;
        assert_eq!(
            store.check_at(&token, now).unwrap().code,
            "total_quota_exceeded"
        );
        token.used_total = 0;
        token.used_today = 0;
        assert_eq!(
            store.check_at(&token, now + 60_000).unwrap().code,
            "token_expired"
        );
        token.revoked = true;
        assert_eq!(
            store.check_at(&token, now).unwrap().message,
            "课堂令牌已作废"
        );
    }

    #[test]
    fn a_new_day_resets_the_daily_count() {
        let dir = tempfile::tempdir().unwrap();
        let now = 1_762_400_000_000;
        let store = store(&dir, now);
        let (mut token, _plain) = token("学员 01");
        token.daily_limit = Some(5);
        token.used_today = 5;
        token.used_total = 5;
        token.usage_day = store.day_at(now);
        assert!(store.check_at(&token, now).is_some());
        assert!(
            store.check_at(&token, now + 86_400_000).is_none(),
            "yesterday's use does not count today"
        );
    }
}
