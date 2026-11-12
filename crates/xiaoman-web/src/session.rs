//! Session state: memory, pending answers, secrets to redact, and the agent rebuilt when the model
//! or MCP configuration changes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use xiaoman_agent::prompts::NO_PHONE_PERSONA;
use xiaoman_agent::{AgentDeps, XiaomanAgent};
use xiaoman_api::{
    Action, ActionResult, AgentConfig, AppInfo, ApprovalAnswer, ApprovalError, ApprovalGateway,
    ApprovalRequest, AuditRecord, AuditSink, Device, DeviceError, LlmClient, ModelConfig,
    ModelRole, ScreenState, Skill, SkillChannel, SkillLoadResult, SkillRepository, StopSignal,
};
use xiaoman_llm::{ClientOptions, OpenAiCompatibleClient};
use xiaoman_mcp::McpConfig;
use xiaoman_sentinel::PolicySentinel;
use xiaoman_skills::DirectorySkillRepository;

use crate::channels::WebChannels;
use crate::ics::IcsReminders;
use crate::mcp_provider::{McpToolProvider, TransportFactory, build_provider};
use crate::memory::{Clock, InMemoryMemory};
use crate::problem::Problem;
use crate::random;
use crate::safe_dns::SafeDns;

/// Builds the model clients; tests replace it with a scripted model.
pub type LlmFactory = Arc<dyn Fn(&ModelConfig) -> Arc<dyn LlmClient> + Send + Sync>;

const MIN_SECRET_CHARS: usize = 6;

/// OpenAI-compatible clients that only reach public addresses unless `allow_private` is set.
pub fn llm_factory(allow_private: bool) -> LlmFactory {
    let http = reqwest::Client::builder()
        .dns_resolver(Arc::new(SafeDns { allow_private }))
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap_or_default();
    Arc::new(move |config: &ModelConfig| {
        Arc::new(OpenAiCompatibleClient::with_options(
            config.clone(),
            ClientOptions {
                http: Some(http.clone()),
                ..Default::default()
            },
        )) as Arc<dyn LlmClient>
    })
}

struct Prepared {
    fingerprint: String,
    agent: Arc<XiaomanAgent>,
    provider: Arc<McpToolProvider>,
}

pub struct WebSession {
    id: String,
    last_used: AtomicI64,
    memory: Arc<InMemoryMemory>,
    channels: Arc<WebChannels>,
    busy: AtomicBool,
    closed: AtomicBool,
    prepared: Mutex<Option<Prepared>>,
    secrets: Mutex<Vec<String>>,
    turn: Mutex<Option<tokio::task::AbortHandle>>,
}

/// Everything one turn needs to rebuild the agent: the request body and the pieces of the host.
pub struct AssembleInputs<'a> {
    pub body: &'a Value,
    pub config: &'a McpConfig,
    pub llm: &'a LlmFactory,
    pub transports: &'a TransportFactory,
    pub skills: Arc<dyn SkillRepository>,
    pub offset_minutes: i32,
    pub clock: Clock,
}

impl WebSession {
    pub fn new(id: String, now: i64, clock: Clock) -> Self {
        Self {
            id,
            last_used: AtomicI64::new(now),
            memory: Arc::new(InMemoryMemory::new(clock)),
            channels: Arc::new(WebChannels::new()),
            busy: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            prepared: Mutex::new(None),
            secrets: Mutex::new(Vec::new()),
            turn: Mutex::new(None),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn last_used(&self) -> i64 {
        self.last_used.load(Ordering::SeqCst)
    }

    pub fn touch(&self, now: i64) {
        self.last_used.store(now, Ordering::SeqCst);
    }

    pub fn memory(&self) -> &Arc<InMemoryMemory> {
        &self.memory
    }

    pub fn channels(&self) -> &Arc<WebChannels> {
        &self.channels
    }

    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Marks the turn as running; false when another message is still being handled.
    pub fn begin_turn(&self) -> bool {
        self.busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    pub fn finish_turn(&self, now: i64) {
        *self.turn.lock().unwrap() = None;
        // Detach before releasing the busy flag: the next turn attaches its own stream, and it must
        // not be cleared by the tail of this one.
        self.channels.detach();
        if !self.closed() {
            self.touch(now);
        }
        self.busy.store(false, Ordering::SeqCst);
    }

    pub fn set_turn(&self, handle: tokio::task::AbortHandle) {
        let previous = self.turn.lock().unwrap().replace(handle);
        drop(previous);
    }

    /// Remembers every credential the request carries so events can be redacted.
    pub fn remember_secrets(&self, body: &Value) {
        let mut values: Vec<String> = Vec::new();
        let model = object(body, "model");
        values.push(string(model, "apiKey"));
        values.push(string(model, "classToken"));
        let mcp = object(body, "mcp");
        if let Some(keys) = mcp
            .and_then(|mcp| mcp.get("keys"))
            .and_then(Value::as_object)
        {
            values.extend(keys.values().filter_map(primitive_text));
        }
        if let Some(custom) = mcp
            .and_then(|mcp| mcp.get("custom"))
            .and_then(Value::as_array)
        {
            for item in custom {
                let Some(item) = item.as_object() else {
                    continue;
                };
                if let Some(headers) = item.get("headers").and_then(Value::as_object) {
                    values.extend(headers.values().filter_map(primitive_text));
                }
                if let Ok(parsed) = url::Url::parse(&string(Some(item), "url")) {
                    if let Some(password) = parsed.password() {
                        values.push(password.to_string());
                    }
                    values.extend(parsed.query_pairs().map(|(_, value)| value.into_owned()));
                }
            }
        }
        // "Bearer abc" is stored both whole and without the scheme.
        let extended: Vec<String> = values
            .iter()
            .filter_map(|value| bearer_token(value))
            .collect();
        values.extend(extended);
        let mut secrets = self.secrets.lock().unwrap();
        for value in values
            .into_iter()
            .filter(|value| value.chars().count() >= MIN_SECRET_CHARS)
        {
            push_unique(&mut secrets, value.clone());
            push_unique(&mut secrets, json_escaped(&value));
        }
    }

    pub fn remember_secret(&self, value: &str) {
        if value.chars().count() >= MIN_SECRET_CHARS {
            push_unique(&mut self.secrets.lock().unwrap(), value.to_string());
        }
    }

    /// Replaces every remembered credential with `***`, longest first, in strings and keys.
    pub fn redact_json(&self, value: &Value) -> Value {
        let secrets = self.secrets.lock().unwrap();
        redact_value(value, &secrets)
    }

    pub fn redact_text(&self, text: &str) -> String {
        let secrets = self.secrets.lock().unwrap();
        redact_text(text, &secrets)
    }

    /// Rebuilds the agent only when the model or MCP configuration changed.
    pub fn assemble(&self, input: AssembleInputs<'_>) -> Result<Arc<XiaomanAgent>, Problem> {
        let AssembleInputs {
            body,
            config,
            llm,
            transports,
            skills,
            offset_minutes,
            clock,
        } = input;
        if self.closed() {
            return Err(Problem::not_found("会话已过期"));
        }
        let fingerprint = serde_json::to_string(&json!({
            "model": body.get("model"),
            "mcp": body.get("mcp"),
        }))
        .unwrap_or_default();
        if let Some(prepared) = self.prepared.lock().unwrap().as_ref() {
            if prepared.fingerprint == fingerprint {
                return Ok(prepared.agent.clone());
            }
        }
        let model = object(body, "model");
        let api_key = string(model, "apiKey");
        let chat_config = ModelConfig::new(
            ModelRole::Chat,
            string(model, "baseUrl"),
            string(model, "model"),
            api_key,
        );
        let chat = (llm)(&chat_config);
        let operator = (llm)(&ModelConfig {
            role: ModelRole::Operator,
            ..chat_config
        });

        let provider = Arc::new(build_provider(
            config.clone(),
            self.channels.clone(),
            transports,
            &clock,
        ));
        let agent = Arc::new(
            XiaomanAgent::new(
                AgentDeps {
                    chat,
                    operator,
                    device: Arc::new(NoPhone),
                    memory: self.memory.clone(),
                    skills,
                    sentinel: Arc::new(PolicySentinel::new(
                        Arc::new(DenyApprovals),
                        Arc::new(NoAudit),
                    )),
                    users: self.channels.clone(),
                    reminders: Arc::new(IcsReminders::new(
                        self.channels.clone(),
                        offset_minutes,
                        clock.clone(),
                    )),
                },
                AgentConfig {
                    phone_tasks: false,
                    ..Default::default()
                },
            )
            .with_persona(NO_PHONE_PERSONA)
            .with_external_tools(provider.clone())
            .with_clock(clock),
        );
        let mut prepared = self.prepared.lock().unwrap();
        if let Some(previous) = prepared.take() {
            previous.provider.close();
        }
        *prepared = Some(Prepared {
            fingerprint,
            agent: agent.clone(),
            provider,
        });
        Ok(agent)
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(turn) = self.turn.lock().unwrap().take() {
            turn.abort();
        }
        self.channels.detach();
        if let Some(prepared) = self.prepared.lock().unwrap().take() {
            prepared.provider.close();
        }
        self.memory.clear();
    }
}

/// Keeps the agent's stop check in step with the session.
pub struct SessionStop {
    session: Arc<WebSession>,
}

impl SessionStop {
    pub fn new(session: Arc<WebSession>) -> Self {
        Self { session }
    }
}

impl StopSignal for SessionStop {
    fn stopped(&self) -> bool {
        self.session.closed()
    }
}

/// Skills loaded for the web: only chat workflows reach a web session.
pub struct ChatSkillRepository {
    delegate: DirectorySkillRepository,
}

impl ChatSkillRepository {
    pub fn new(root: Option<std::path::PathBuf>) -> Self {
        Self {
            delegate: DirectorySkillRepository::new(root.into_iter().collect()),
        }
    }
}

fn is_chat(skill: &Skill) -> bool {
    skill.channel == SkillChannel::Chat
}

#[async_trait]
impl SkillRepository for ChatSkillRepository {
    async fn reload(&self) -> SkillLoadResult {
        let mut result = self.delegate.reload().await;
        result.skills.retain(is_chat);
        result
    }

    async fn list(&self) -> Vec<Skill> {
        self.delegate
            .list()
            .await
            .into_iter()
            .filter(is_chat)
            .collect()
    }

    async fn get(&self, name: &str) -> Option<Skill> {
        self.delegate.get(name).await.filter(is_chat)
    }
}

/// The web cannot operate a phone.
struct NoPhone;

#[async_trait]
impl Device for NoPhone {
    async fn observe(&self) -> Result<ScreenState, DeviceError> {
        Err(DeviceError("网页版不能操作手机".to_string()))
    }

    async fn perform(&self, _action: Action) -> ActionResult {
        ActionResult::failed("网页版不能操作手机")
    }

    async fn installed_apps(&self) -> Vec<AppInfo> {
        Vec::new()
    }
}

/// Phone approvals never apply here: the web agent has no device actions.
struct DenyApprovals;

#[async_trait]
impl ApprovalGateway for DenyApprovals {
    async fn request(&self, _request: ApprovalRequest) -> Result<ApprovalAnswer, ApprovalError> {
        Ok(ApprovalAnswer::once(false))
    }
}

struct NoAudit;

impl AuditSink for NoAudit {
    fn record(&self, _record: AuditRecord) {}
}

/// Sessions expire after `session_millis` without use and are capped at `max_sessions` (least
/// recently used goes first).
pub struct SessionStore {
    sessions: HashMap<String, Arc<WebSession>>,
    max_sessions: usize,
    session_millis: i64,
    clock: Clock,
}

impl SessionStore {
    pub fn new(max_sessions: usize, session_millis: i64, clock: Clock) -> Self {
        Self {
            sessions: HashMap::new(),
            max_sessions,
            session_millis,
            clock,
        }
    }

    pub fn session(&mut self, id: &str) -> Option<Arc<WebSession>> {
        self.expire();
        let session = self.sessions.get(id)?.clone();
        session.touch((self.clock)());
        Some(session)
    }

    pub fn new_session(&mut self) -> Arc<WebSession> {
        self.expire();
        while self.sessions.len() >= self.max_sessions.max(1) {
            let oldest = self
                .sessions
                .values()
                .min_by_key(|session| session.last_used())
                .map(|session| session.id().to_string());
            let Some(oldest) = oldest else { break };
            if let Some(session) = self.sessions.remove(&oldest) {
                session.close();
            }
        }
        let session = Arc::new(WebSession::new(
            random::session_id(),
            (self.clock)(),
            self.clock.clone(),
        ));
        self.sessions
            .insert(session.id().to_string(), session.clone());
        session
    }

    pub fn expire(&mut self) {
        let now = (self.clock)();
        let expired: Vec<String> = self
            .sessions
            .values()
            .filter(|session| now - session.last_used() >= self.session_millis)
            .map(|session| session.id().to_string())
            .collect();
        for id in expired {
            if let Some(session) = self.sessions.remove(&id) {
                session.close();
            }
        }
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn close_all(&mut self) {
        for (_, session) in self.sessions.drain() {
            session.close();
        }
    }
}

fn object<'a>(value: &'a Value, key: &str) -> Option<&'a Map<String, Value>> {
    value.get(key).and_then(Value::as_object)
}

fn string(object: Option<&Map<String, Value>>, key: &str) -> String {
    object
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn primitive_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn bearer_token(value: &str) -> Option<String> {
    let lower = value.to_lowercase();
    lower.starts_with("bearer ").then(|| value[7..].to_string())
}

fn json_escaped(value: &str) -> String {
    let quoted = Value::String(value.to_string()).to_string();
    quoted[1..quoted.len() - 1].to_string()
}

fn push_unique(secrets: &mut Vec<String>, value: String) {
    if !value.is_empty() && !secrets.contains(&value) {
        secrets.push(value);
    }
}

fn redact_text(text: &str, secrets: &[String]) -> String {
    let mut ordered: Vec<&String> = secrets.iter().collect();
    ordered.sort_by_key(|secret| std::cmp::Reverse(secret.chars().count()));
    let mut out = text.to_string();
    for secret in ordered {
        out = out.replace(secret.as_str(), "***");
    }
    out
}

fn redact_value(value: &Value, secrets: &[String]) -> Value {
    match value {
        Value::String(text) => Value::String(redact_text(text, secrets)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_value(item, secrets))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (redact_text(key, secrets), redact_value(value, secrets)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Sessions expire after `session_millis` without use and are capped at `max_sessions` (least
/// recently used goes first).
#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> WebSession {
        WebSession::new("test".to_string(), 0, Arc::new(|| 123))
    }

    #[test]
    fn secrets_are_redacted_including_quoted_and_bearer_forms() {
        let session = session();
        let key = "quoted\"secret-123";
        session.remember_secrets(&json!({
            "model": { "apiKey": key, "classToken": "" },
            "mcp": {
                "keys": { "AMAP_MAPS_API_KEY": "mcp-secret-456" },
                "custom": [{
                    "url": "https://example.com/mcp?token=url-secret-789",
                    "headers": { "Authorization": "Bearer header-secret-012" },
                }],
            },
        }));
        for secret in [
            "quoted\"secret-123",
            "mcp-secret-456",
            "url-secret-789",
            "header-secret-012",
        ] {
            assert!(
                !session
                    .redact_text(&format!("值 {secret} 完"))
                    .contains(secret),
                "{secret}"
            );
            assert_eq!(session.redact_text(&format!("值 {secret} 完")), "值 *** 完");
        }
        let arguments = json!({ "key": key }).to_string();
        let redacted = session.redact_json(&json!({ "type": "approval", "arguments": arguments }));
        let text = redacted["arguments"].as_str().unwrap();
        assert!(!text.contains(key));
        assert!(!text.contains(&json_escaped(key)));
    }

    #[test]
    fn short_values_are_not_secrets() {
        let session = session();
        session.remember_secrets(&json!({ "model": { "apiKey": "short" } }));
        assert_eq!(session.redact_text("a short reply"), "a short reply");
        session.remember_secret("a");
        assert_eq!(session.redact_text("a cat"), "a cat");
    }

    #[test]
    fn sessions_are_capped_and_expire() {
        let now = std::sync::Arc::new(AtomicI64::new(100));
        let clock: Clock = {
            let now = now.clone();
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let mut store = SessionStore::new(2, 100, clock);
        let first = store.new_session();
        assert_eq!(first.id().len(), 22);
        now.store(110, Ordering::SeqCst);
        let second = store.new_session();
        now.store(120, Ordering::SeqCst);
        let third = store.new_session();
        assert!(first.closed());
        assert!(!second.closed());
        assert_eq!(store.len(), 2);
        assert!(store.session(first.id()).is_none());
        assert!(store.session(third.id()).is_some());
        now.store(260, Ordering::SeqCst);
        store.expire();
        assert_eq!(store.len(), 0);
        assert!(second.closed() && third.closed());
    }

    #[tokio::test]
    async fn only_chat_skills_reach_the_web_agent() {
        let repository = ChatSkillRepository::new(None);
        assert!(repository.list().await.is_empty());
        assert!(repository.get("missing").await.is_none());
    }
}
