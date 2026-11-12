//! The one conversation with the agent: a chat turn at a time, parked questions and approvals, and
//! the persisted stores it runs on. Mirrors `app/src/main/kotlin/dev/xiaoman/app/session/AgentSession.kt`
//! plus the parts of `AppGraph.kt` that build the agent and its stores.
//!
//! The session owns a small tokio runtime: [`Session::send`] starts one turn on it, the turn's
//! [`AgentListener`] maps every [`AgentEvent`] to a [`ChatItem`] (the same mapping the chat screen
//! uses), and subscribers receive the raw events through [`Session::subscribe`].
//!
//! [`Session`] is the agent's [`UserChannel`] and [`ApprovalGateway`]: `ask` and `request` park on a
//! one-shot channel until the UI calls [`Session::answer_question`] or
//! [`Session::answer_approval`], and [`Session::stop`] releases both the way the Kotlin session
//! does (the question answers 已停止, the approval is denied).
//!
//! Everything persistent lives under [`SessionConfig::data_dir`]: `settings.json`,
//! `settings-secrets.json`, `mcp.json`, `memory/` and `traces/`, plus `skills/` for user skills.

use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{broadcast, oneshot};
use xiaoman_agent::prompts::NO_PHONE_PERSONA;
use xiaoman_agent::{AgentDeps, XiaomanAgent};
use xiaoman_api::{
    Action, ActionResult, Agent, AgentConfig, AgentEvent, AgentListener, AppInfo, ApprovalAnswer,
    ApprovalError, ApprovalGateway, ApprovalRequest, ApprovalScope, Device, DeviceError, LlmClient,
    MemoryError, MemoryStore, ModelConfig, ModelRole, Note, ReminderScheduler, ScreenState,
    SearchHit, Skill, SkillLoadResult, SkillRepository, StopSignal, UserChannel,
};
use xiaoman_llm::OpenAiCompatibleClient;
use xiaoman_mcp::{
    McpCatalog, McpClient, McpConfig, McpPlatform, McpSelection, McpToolProvider, ServerStatus,
    UserChannelApprovals,
};
use xiaoman_memory::FileMemoryStore;
use xiaoman_sentinel::PolicySentinel;
use xiaoman_skills::DirectorySkillRepository;

use crate::chat_item::{ApprovalTexts, ChatItem, approval_line, step_text};
use crate::mcp_settings::McpSettingsStore;
use crate::run_ids::RunIds;
use crate::settings::{AppSettings, SettingsFactory, SettingsStore};
use crate::trace::{RecordingDevice, TraceWriter};

/// Adds the hand-written `mcpServers` of `mcp.json` to a catalog config, like the Kotlin
/// `AppGraph.mcpConfig`: only enabled HTTP servers, and never one the catalog already named.
fn merge_file_servers(config: &mut McpConfig, data_dir: &Path) {
    let file = McpConfig::load(&data_dir.join("mcp.json"), &Default::default());
    for server in file.servers {
        if !server.enabled || server.url.is_none() {
            continue;
        }
        if config.servers.iter().any(|known| known.name == server.name) {
            continue;
        }
        config.servers.push(server);
    }
}

/// The answer a parked question gets when the run stops or ends without an answer.
pub const STOPPED_TEXT: &str = "已停止";
/// Why a parked approval was released without an answer.
pub const APPROVAL_CANCELLED: &str = "已取消";
/// The desktop has no phone to operate; every device action fails with this.
pub const DEVICE_UNSUPPORTED: &str = "这台电脑上不能操作手机。";
/// Shown when a run cannot start because the two models are not configured.
pub const NOT_CONFIGURED_TEXT: &str = "还没有配置模型，请先在设置里填写聊天模型和操作模型。";
/// Shown when a run is rejected because the previous one is still in flight.
pub const BUSY_TEXT: &str = "上一条消息还在处理，请稍后再试。";

/// `AgentSession.MAX_ITEMS`: the transcript keeps the newest rows only.
pub const MAX_ITEMS: usize = 500;
/// Subscribers that fall this far behind lose their oldest events instead of blocking a turn.
const EVENT_BUFFER: usize = 256;

/// Builds the model clients; tests replace it with a scripted model.
pub type LlmFactory = Arc<dyn Fn(&ModelConfig) -> Arc<dyn LlmClient> + Send + Sync>;

/// Receives every trace line, e.g. logcat on Android.
pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

/// OpenAI-compatible clients for the settings' endpoints.
pub fn openai_clients() -> LlmFactory {
    Arc::new(|config: &ModelConfig| {
        Arc::new(OpenAiCompatibleClient::new(config.clone())) as Arc<dyn LlmClient>
    })
}

// ---------------------------------------------------------------------------------------------
// Configuration and errors
// ---------------------------------------------------------------------------------------------

/// Where the session keeps everything and how it reaches the models.
#[derive(Clone)]
pub struct SessionConfig {
    /// One directory for `settings.json`, `settings-secrets.json`, `mcp.json`, `memory/`, `skills/`
    /// and `traces/`.
    pub data_dir: PathBuf,
    /// Extra skill roots searched before the user's `skills/` folder, so a user skill wins.
    pub extra_skill_roots: Vec<PathBuf>,
    /// Builds the chat and operator clients from the persisted settings.
    pub llm_factory: LlmFactory,
    /// The platform's hands; `None` is the desktop, where `phone_task` is not offered.
    pub device: Option<Arc<dyn Device>>,
    /// Where `reminder_set` goes; `None` only records the reminder in this session.
    pub reminders: Option<Arc<dyn ReminderScheduler>>,
    /// Which catalog entries and confirm policies the MCP selection may use.
    pub platform: McpPlatform,
    /// Persona for the chat model; `None` keeps the agent's default (or the no-phone one).
    pub persona: Option<String>,
    /// Receives every trace line, e.g. logcat on Android; `None` writes files only.
    pub log_sink: Option<LogSink>,
    /// Where the settings live; `None` opens `settings.json`/`settings-secrets.json` under
    /// `data_dir`. Android passes a Keystore-backed secret half.
    pub settings_factory: Option<SettingsFactory>,
}

impl SessionConfig {
    /// A session under `data_dir` with the real OpenAI-compatible clients.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            extra_skill_roots: Vec::new(),
            llm_factory: openai_clients(),
            device: None,
            reminders: None,
            platform: McpPlatform::Desktop,
            persona: None,
            log_sink: None,
            settings_factory: None,
        }
    }

    /// Replaces the settings store, e.g. with one whose secret half is the Android Keystore.
    pub fn with_settings_factory(mut self, factory: SettingsFactory) -> Self {
        self.settings_factory = Some(factory);
        self
    }

    pub fn with_llm_factory(mut self, factory: LlmFactory) -> Self {
        self.llm_factory = factory;
        self
    }

    /// Adds a skill root searched before the user's own folder, e.g. the bundled examples.
    pub fn with_skill_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.extra_skill_roots.push(root.into());
        self
    }

    /// Hands the session a phone; `phone_task` becomes available to the chat model.
    pub fn with_device(mut self, device: Arc<dyn Device>) -> Self {
        self.device = Some(device);
        self
    }

    /// Sends `reminder_set` to the platform instead of only recording it.
    pub fn with_reminders(mut self, reminders: Arc<dyn ReminderScheduler>) -> Self {
        self.reminders = Some(reminders);
        self
    }

    pub fn with_platform(mut self, platform: McpPlatform) -> Self {
        self.platform = platform;
        self
    }

    /// Replaces the default persona (and the desktop no-phone persona).
    pub fn with_persona(mut self, persona: impl Into<String>) -> Self {
        self.persona = Some(persona.into());
        self
    }

    /// Receives every trace line, e.g. to mirror them into logcat.
    pub fn with_log_sink(mut self, sink: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.log_sink = Some(Arc::new(sink));
        self
    }

    /// The OS convention: `%APPDATA%\xiaoman` on Windows, `$XDG_DATA_HOME/xiaoman` (or
    /// `~/.local/share/xiaoman`) elsewhere, and `xiaoman-data` when neither is set.
    pub fn default_data_dir() -> PathBuf {
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
                })
        };
        base.map(|base| base.join("xiaoman"))
            .unwrap_or_else(|| PathBuf::from("xiaoman-data"))
    }
}

impl fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionConfig")
            .field("data_dir", &self.data_dir)
            .field("extra_skill_roots", &self.extra_skill_roots)
            .field("llm_factory", &"<factory>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionError(pub String);

impl SessionError {
    fn io(context: &str, error: io::Error) -> Self {
        Self(format!("{context}: {error}"))
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SessionError {}

/// What a chat input did, the Kotlin `AgentSession.Submit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// The text answered a pending question.
    Answered,
    /// A new turn started.
    Started,
    Busy,
    NotConfigured,
    Empty,
}

impl SendOutcome {
    /// The input clears unless the session was busy or the text was blank.
    pub fn clears_draft(self) -> bool {
        !matches!(self, Self::Busy | Self::Empty)
    }
}

/// One prompt the agent is waiting on, for the chat screen's question row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingQuestion {
    pub id: i64,
    pub task_id: String,
    pub text: String,
    pub takeover: bool,
}

/// One approval the agent is waiting on, for the chat screen's approval card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub id: i64,
    pub task_id: String,
    pub prompt: String,
    pub reason: String,
}

/// Everything the screens read from the session at one moment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionSnapshot {
    pub items: Vec<ChatItem>,
    pub busy: bool,
    pub status: Option<String>,
    pub question: Option<PendingQuestion>,
    pub approval: Option<PendingApproval>,
    pub configured: bool,
}

// ---------------------------------------------------------------------------------------------
// Shared state: the transcript, the run flag and the parked prompts
// ---------------------------------------------------------------------------------------------

struct QuestionSlot {
    id: i64,
    task_id: String,
    text: String,
    takeover: bool,
    answer: Option<oneshot::Sender<String>>,
}

struct ApprovalSlot {
    id: i64,
    task_id: String,
    prompt: String,
    reason: String,
    answer: Option<oneshot::Sender<ApprovalAnswer>>,
}

struct State {
    items: Vec<ChatItem>,
    next_id: i64,
    busy: bool,
    status: Option<String>,
    question: Option<QuestionSlot>,
    approval: Option<ApprovalSlot>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
            busy: false,
            status: None,
            question: None,
            approval: None,
        }
    }
}

impl State {
    fn take_id(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn push(&mut self, item: ChatItem) {
        self.items.push(item);
        if self.items.len() > MAX_ITEMS {
            let overflow = self.items.len() - MAX_ITEMS;
            self.items.drain(..overflow);
        }
    }
}

/// The state and wakeups both the public [`Session`] and the agent's channels hold.
struct Shared {
    state: Mutex<State>,
    /// Signalled whenever a turn ends.
    finished: Condvar,
    events: broadcast::Sender<SessionEvent>,
    stop: AtomicBool,
}

impl Shared {
    fn new() -> Self {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Self {
            state: Mutex::new(State::default()),
            finished: Condvar::new(),
            events,
            stop: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic on a turn's thread must not take the transcript down with it.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Marks the run as running; false while another turn is still in flight.
    fn begin_run(&self) -> bool {
        let mut state = self.lock();
        if state.busy {
            return false;
        }
        self.stop.store(false, Ordering::SeqCst);
        state.busy = true;
        state.status = None;
        drop(state);
        self.changed();
        true
    }

    fn finish(&self) {
        let mut state = self.lock();
        // Dropping the senders wakes `ask`/`request` if the turn panicked while a prompt was open.
        state.question = None;
        state.approval = None;
        state.busy = false;
        state.status = None;
        drop(state);
        self.changed();
        self.finished.notify_all();
    }

    /// Tells subscribers to re-read the state; no subscriber is fine.
    fn changed(&self) {
        let _ = self.events.send(SessionEvent::State);
    }

    fn stop(&self) {
        let mut state = self.lock();
        if !state.busy {
            return;
        }
        self.stop.store(true, Ordering::SeqCst);
        if let Some(slot) = state.question.take() {
            if let Some(sender) = slot.answer {
                let _ = sender.send(STOPPED_TEXT.to_string());
            }
        }
        if let Some(slot) = state.approval.take() {
            if let Some(sender) = slot.answer {
                let _ = sender.send(ApprovalAnswer::once(false));
            }
        }
        state.status = Some(STOPPED_TEXT.to_string());
        drop(state);
        self.changed();
    }

    fn answer_question(&self, text: &str) -> bool {
        let message = text.trim();
        if message.is_empty() {
            return false;
        }
        let sender = {
            let mut state = self.lock();
            let Some(slot) = state.question.take() else {
                return false;
            };
            slot.answer
        };
        let Some(sender) = sender else { return false };
        let mut state = self.lock();
        let id = state.take_id();
        state.push(ChatItem::User {
            id,
            text: message.to_string(),
        });
        drop(state);
        self.changed();
        sender.send(message.to_string()).is_ok()
    }

    /// Answers the parked approval only when `approval_id` is the one on screen.
    ///
    /// A card or notification that outlived its approval must not answer a newer one, so a stale
    /// id returns false and leaves the current prompt untouched.
    fn answer_approval(&self, approval_id: i64, answer: ApprovalAnswer) -> bool {
        let sender = {
            let mut state = self.lock();
            if !state
                .approval
                .as_ref()
                .is_some_and(|slot| slot.id == approval_id)
            {
                return false;
            }
            let Some(slot) = state.approval.take() else {
                return false;
            };
            slot.answer
        };
        match sender {
            Some(sender) => {
                self.changed();
                sender.send(answer).is_ok()
            }
            None => false,
        }
    }

    async fn ask(&self, task_id: &str, question: &str, takeover: bool) -> String {
        if self.stopped() {
            return STOPPED_TEXT.to_string();
        }
        let (sender, receiver) = oneshot::channel();
        let id = {
            let mut state = self.lock();
            let id = state.take_id();
            state.question = Some(QuestionSlot {
                id,
                task_id: task_id.to_string(),
                text: question.to_string(),
                takeover,
                answer: Some(sender),
            });
            state.status = Some(if takeover {
                "请接管手机，完成后回到小满回复".to_string()
            } else {
                format!("等待你回答：{question}")
            });
            id
        };
        self.changed();
        let answer = match receiver.await {
            Ok(answer) => answer,
            Err(_) => STOPPED_TEXT.to_string(),
        };
        let mut state = self.lock();
        if state.question.as_ref().is_some_and(|slot| slot.id == id) {
            state.question = None;
        }
        drop(state);
        self.changed();
        answer
    }

    async fn request(
        &self,
        request: ApprovalRequest,
        texts: &ApprovalTexts,
    ) -> Result<ApprovalAnswer, ApprovalError> {
        if self.stopped() {
            return Err(ApprovalError(STOPPED_TEXT.to_string()));
        }
        let (sender, receiver) = oneshot::channel();
        let (id, previous) = {
            let mut state = self.lock();
            let id = state.take_id();
            let previous = state.status.clone();
            state.approval = Some(ApprovalSlot {
                id,
                task_id: request.task_id.clone(),
                prompt: request.prompt.clone(),
                reason: request.reason.clone(),
                answer: Some(sender),
            });
            state.status = Some(format!("等待确认：{}", request.prompt));
            (id, previous)
        };
        self.changed();
        let answer = receiver.await;
        let mut state = self.lock();
        if state.approval.as_ref().is_some_and(|slot| slot.id == id) {
            state.approval = None;
        }
        let result = match answer {
            Ok(answer) => {
                let line_id = state.take_id();
                let line = approval_line(line_id, &request.prompt, answer, texts);
                state.push(line);
                if !self.stopped() {
                    state.status = previous;
                }
                Ok(answer)
            }
            Err(_) => {
                if !self.stopped() {
                    state.status = previous;
                }
                Err(ApprovalError(APPROVAL_CANCELLED.to_string()))
            }
        };
        drop(state);
        self.changed();
        result
    }

    fn snapshot(&self, configured: bool) -> SessionSnapshot {
        let state = self.lock();
        SessionSnapshot {
            items: state.items.clone(),
            busy: state.busy,
            status: state.status.clone(),
            question: state.question.as_ref().map(|slot| PendingQuestion {
                id: slot.id,
                task_id: slot.task_id.clone(),
                text: slot.text.clone(),
                takeover: slot.takeover,
            }),
            approval: state.approval.as_ref().map(|slot| PendingApproval {
                id: slot.id,
                task_id: slot.task_id.clone(),
                prompt: slot.prompt.clone(),
                reason: slot.reason.clone(),
            }),
            configured,
        }
    }
}

/// Only parks approvals; the sentinel is built before the full channel exists, and both answer
/// through the same [`Shared`].
struct Approvals {
    shared: Arc<Shared>,
    texts: ApprovalTexts,
}

#[async_trait]
impl ApprovalGateway for Approvals {
    async fn request(&self, request: ApprovalRequest) -> Result<ApprovalAnswer, ApprovalError> {
        self.shared.request(request, &self.texts).await
    }
}

/// What [`EventStream`] delivers: the agent's own events, plus a nudge whenever the run flag, the
/// status or a pending prompt changed, so a screen can re-read [`Session::state`].
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Agent(AgentEvent),
    /// Re-read the session state: a turn ended, or a prompt was answered or released.
    State,
}

/// A subscriber's view of the session's events, over a tokio broadcast channel.
///
/// A subscriber that falls too far behind loses its oldest events instead of blocking a turn; the
/// screen then re-reads [`Session::state`], which always has the current transcript.
pub struct EventStream {
    receiver: broadcast::Receiver<SessionEvent>,
}

impl EventStream {
    /// The next event, or `None` once the session is gone.
    pub async fn recv(&mut self) -> Option<SessionEvent> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// The next event without waiting, or `None` when none is buffered.
    pub fn try_recv(&mut self) -> Option<SessionEvent> {
        loop {
            match self.receiver.try_recv() {
                Ok(event) => return Some(event),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => return None,
            }
        }
    }
}

/// The agent's [`UserChannel`], [`ApprovalGateway`], [`StopSignal`] and [`AgentListener`], all on
/// top of one [`Shared`]. The session hands these to the agent so the agent may hold them without
/// holding the session.
struct SessionChannels {
    shared: Arc<Shared>,
    trace: Arc<TraceWriter>,
    sentinel: Arc<PolicySentinel>,
    texts: ApprovalTexts,
}

impl SessionChannels {
    fn trace_first(&self, event: &AgentEvent) {
        // A full disk must not stop the task: TraceWriter reports failures through its warning sink.
        self.trace.on_event(event);
        if let AgentEvent::PhoneTaskFinished { task_id, .. } = event {
            self.sentinel.end_task(task_id);
        }
    }
}

impl AgentListener for SessionChannels {
    fn on_event(&self, event: &AgentEvent) {
        self.trace_first(event);
        {
            let mut state = self.shared.lock();
            if let Some(status) = step_text::status(event) {
                state.status = Some(status);
            }
            let id = state.take_id();
            if let Some(item) = step_text::chat_item(event, id) {
                state.push(item);
            }
        }
        // No subscribers is fine: the transcript above still has the event.
        let _ = self.shared.events.send(SessionEvent::Agent(event.clone()));
    }
}

impl StopSignal for SessionChannels {
    fn stopped(&self) -> bool {
        self.shared.stopped()
    }
}

#[async_trait]
impl UserChannel for SessionChannels {
    async fn ask(&self, task_id: &str, question: &str, takeover: bool) -> String {
        self.shared.ask(task_id, question, takeover).await
    }
}

#[async_trait]
impl ApprovalGateway for SessionChannels {
    async fn request(&self, request: ApprovalRequest) -> Result<ApprovalAnswer, ApprovalError> {
        self.shared.request(request, &self.texts).await
    }
}

/// The desktop's hands: there is no phone, so every action fails and nothing can be observed.
struct DesktopDevice;

#[async_trait]
impl Device for DesktopDevice {
    async fn observe(&self) -> Result<ScreenState, DeviceError> {
        Err(DeviceError(DEVICE_UNSUPPORTED.to_string()))
    }

    async fn perform(&self, _action: Action) -> ActionResult {
        ActionResult::failed(DEVICE_UNSUPPORTED)
    }

    async fn installed_apps(&self) -> Vec<AppInfo> {
        Vec::new()
    }
}

/// Remembers the reminders the agent sets and, when wrapped around a platform scheduler, also
/// hands them to it.
#[derive(Default)]
pub struct RecordingReminders {
    scheduled: Mutex<Vec<(String, i64)>>,
    inner: Option<Arc<dyn ReminderScheduler>>,
}

impl RecordingReminders {
    /// Records every reminder and forwards it to `inner`.
    pub fn wrapping(inner: Arc<dyn ReminderScheduler>) -> Self {
        Self {
            scheduled: Mutex::new(Vec::new()),
            inner: Some(inner),
        }
    }

    pub fn scheduled(&self) -> Vec<(String, i64)> {
        self.scheduled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ReminderScheduler for RecordingReminders {
    fn schedule(&self, text: &str, at_millis: i64) {
        self.scheduled
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((text.to_string(), at_millis));
        if let Some(inner) = &self.inner {
            inner.schedule(text, at_millis);
        }
    }
}

impl fmt::Debug for RecordingReminders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordingReminders")
            .field("scheduled", &self.scheduled().len())
            .finish()
    }
}

// ---------------------------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------------------------

struct CachedAgent {
    key: (AppSettings, McpSelection),
    agent: Arc<XiaomanAgent>,
    provider: Option<Arc<McpToolProvider>>,
}

struct Inner {
    data_dir: PathBuf,
    shared: Arc<Shared>,
    channels: Arc<SessionChannels>,
    settings: Mutex<SettingsStore>,
    mcp: Mutex<McpSettingsStore>,
    memory: Arc<FileMemoryStore>,
    skills: Arc<DirectorySkillRepository>,
    sentinel: Arc<PolicySentinel>,
    reminders: Arc<RecordingReminders>,
    trace: Arc<TraceWriter>,
    secrets: Arc<Mutex<Vec<String>>>,
    llm: LlmFactory,
    run_ids: Arc<Mutex<RunIds>>,
    agent: Mutex<Option<CachedAgent>>,
    device: Option<Arc<dyn Device>>,
    platform: McpPlatform,
    persona: Option<String>,
    /// `None` only while [`Inner`] drops: a runtime must not be dropped inside an async context.
    runtime: Option<tokio::runtime::Runtime>,
}

impl Inner {
    fn runtime(&self) -> &tokio::runtime::Runtime {
        self.runtime
            .as_ref()
            .expect("the session runtime lives until Drop")
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(cached) = self.agent.get_mut() {
            if let Some(cached) = cached.take() {
                if let Some(provider) = cached.provider {
                    provider.close();
                }
            }
        }
        // Dropping the runtime from a caller's async context would panic; this never blocks.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// The headless chat session: one agent turn at a time, with the persisted stores behind it.
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

impl Session {
    /// Opens the stores under [`SessionConfig::data_dir`] and builds the agent's dependencies.
    ///
    /// The agent itself is built lazily, and rebuilt when the settings or the MCP selection change,
    /// so saving settings in the UI takes effect on the next turn.
    pub fn new(config: SessionConfig) -> Result<Self, SessionError> {
        let data_dir = config.data_dir;
        create_dir(&data_dir)?;
        let memory_dir = data_dir.join("memory");
        let skills_dir = data_dir.join("skills");
        let traces_dir = data_dir.join("traces");
        for dir in [&memory_dir, &skills_dir, &traces_dir] {
            create_dir(dir)?;
        }

        let settings = match config.settings_factory.as_ref() {
            Some(factory) => factory(&data_dir)
                .map_err(|error| SessionError::io(&data_dir.display().to_string(), error))?,
            None => SettingsStore::open_with(&data_dir, |warning| eprintln!("{warning}"))
                .map_err(|error| SessionError::io(&data_dir.display().to_string(), error))?,
        };
        let mcp = McpSettingsStore::open(&data_dir, settings.secret())
            .map_err(|error| SessionError::io(&data_dir.display().to_string(), error))?;

        let secrets = Arc::new(Mutex::new(settings.current().secrets()));
        let secrets_for_trace = secrets.clone();
        let mut trace = TraceWriter::new(&traces_dir).with_secrets(move || {
            secrets_for_trace
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        });
        if let Some(sink) = config.log_sink.clone() {
            trace = trace.with_log(move |line| sink(line));
        }
        let trace = Arc::new(trace);

        let memory = Arc::new(FileMemoryStore::new(&memory_dir));
        let mut roots: Vec<PathBuf> = config.extra_skill_roots;
        if let Some(examples) = repo_skill_examples() {
            if !roots.contains(&examples) {
                roots.push(examples);
            }
        }
        roots.push(skills_dir);
        let skills = Arc::new(DirectorySkillRepository::new(roots));

        let shared = Arc::new(Shared::new());
        let approvals = Arc::new(Approvals {
            shared: shared.clone(),
            texts: ApprovalTexts::default(),
        });
        let sentinel = Arc::new(PolicySentinel::new(approvals, trace.clone()));
        let channels = Arc::new(SessionChannels {
            shared: shared.clone(),
            trace: trace.clone(),
            sentinel: sentinel.clone(),
            texts: ApprovalTexts::default(),
        });

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("xiaoman-session")
            .enable_all()
            .build()
            .map_err(|error| SessionError(format!("无法启动会话线程：{error}")))?;

        let reminders = Arc::new(match config.reminders.clone() {
            Some(inner) => RecordingReminders::wrapping(inner),
            None => RecordingReminders::default(),
        });

        let inner = Arc::new(Inner {
            data_dir,
            shared,
            channels,
            settings: Mutex::new(settings),
            mcp: Mutex::new(mcp),
            memory,
            skills,
            sentinel,
            reminders,
            trace,
            secrets,
            llm: config.llm_factory,
            run_ids: Arc::new(Mutex::new(RunIds::new())),
            agent: Mutex::new(None),
            device: config.device,
            platform: config.platform,
            persona: config.persona,
            runtime: Some(runtime),
        });
        let session = Self { inner };
        // Load the skills once, so the screen and the system prompt start warm.
        session.blocking(session.inner.skills.reload());
        Ok(session)
    }

    pub fn data_dir(&self) -> &Path {
        &self.inner.data_dir
    }

    pub fn memory_dir(&self) -> PathBuf {
        self.inner.data_dir.join("memory")
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.inner.data_dir.join("skills")
    }

    pub fn traces_dir(&self) -> PathBuf {
        self.inner.data_dir.join("traces")
    }

    /// The transcript, the run flag and the two pending prompts.
    pub fn state(&self) -> SessionSnapshot {
        self.inner.shared.snapshot(self.configured())
    }

    pub fn items(&self) -> Vec<ChatItem> {
        self.inner.shared.lock().items.clone()
    }

    pub fn is_busy(&self) -> bool {
        self.inner.shared.lock().busy
    }

    pub fn configured(&self) -> bool {
        self.settings().configured()
    }

    /// Every agent event of this session, plus a nudge when the run state changes; a slow
    /// subscriber loses its oldest events.
    pub fn subscribe(&self) -> EventStream {
        EventStream {
            receiver: self.inner.shared.events.subscribe(),
        }
    }

    /// The tokio handle the turns run on, for callers that need to await inside the session.
    pub fn handle(&self) -> tokio::runtime::Handle {
        self.inner.runtime().handle().clone()
    }

    /// The agent's question channel, for a caller that wants to answer it itself.
    pub fn user_channel(&self) -> Arc<dyn UserChannel> {
        self.inner.channels.clone()
    }

    /// The agent's approval gateway; the chat screen answers through the session instead.
    pub fn approval_gateway(&self) -> Arc<dyn ApprovalGateway> {
        self.inner.channels.clone()
    }

    /// Chat input: answers a pending question, else starts one chat turn.
    pub fn send(&self, text: &str) -> SendOutcome {
        let message = text.trim();
        if message.is_empty() {
            return SendOutcome::Empty;
        }
        if self.answer_question(message) {
            return SendOutcome::Answered;
        }
        if !self.configured() {
            let mut state = self.inner.shared.lock();
            let id = state.take_id();
            state.push(ChatItem::User {
                id,
                text: message.to_string(),
            });
            let id = state.take_id();
            state.push(ChatItem::Error {
                id,
                text: NOT_CONFIGURED_TEXT.to_string(),
            });
            return SendOutcome::NotConfigured;
        }
        let Some(agent) = self.agent() else {
            return SendOutcome::NotConfigured;
        };
        if !self.inner.shared.begin_run() {
            return SendOutcome::Busy;
        }
        {
            let mut state = self.inner.shared.lock();
            let id = state.take_id();
            state.push(ChatItem::User {
                id,
                text: message.to_string(),
            });
        }
        self.spawn_turn(agent, message.to_string());
        SendOutcome::Started
    }

    /// Runs one phone task directly, the debug bridge's `RUN` and the skill tests' entry point.
    ///
    /// `run_id` must be a valid [`RunIds`] id when given; otherwise the session generates one.
    /// Returns false when the task could not start (no model configured, or a turn is in flight);
    /// either way the run still ends with `Failed` + `PhoneTaskFinished` on the trace, so
    /// `xiaoman run --follow` returns.
    pub fn run_phone_task(&self, goal: &str, skill: Option<&str>, run_id: Option<&str>) -> bool {
        let id = match run_id.filter(|id| RunIds::is_valid(id)) {
            Some(id) => id.to_string(),
            None => self
                .inner
                .run_ids
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .next_id(),
        };
        let goal = goal.trim();
        if goal.is_empty() {
            self.fail(&id, "任务目标不能为空");
            return false;
        }
        let Some(agent) = self.agent() else {
            self.fail(&id, NOT_CONFIGURED_TEXT);
            return false;
        };
        if !self.inner.shared.begin_run() {
            self.fail(&id, BUSY_TEXT);
            return false;
        }
        {
            let mut ids = self
                .inner
                .run_ids
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            ids.prime(id.clone());
        }
        self.spawn_phone_task(agent, goal.to_string(), skill.map(str::to_string));
        true
    }

    /// Ends `run_id` with `Failed` and `PhoneTaskFinished(false)`, the Kotlin `AgentSession.fail`.
    pub fn fail(&self, run_id: &str, message: &str) {
        let listener = &self.inner.channels;
        listener.on_event(&AgentEvent::Failed {
            task_id: run_id.to_string(),
            message: message.to_string(),
        });
        listener.on_event(&AgentEvent::PhoneTaskFinished {
            task_id: run_id.to_string(),
            success: false,
            summary: message.to_string(),
            steps: 0,
        });
    }

    /// Answers the parked question; false when nothing is waiting or the text is blank.
    pub fn answer_question(&self, text: &str) -> bool {
        self.inner.shared.answer_question(text)
    }

    /// Answers the parked approval; false when `approval_id` is not the card on screen.
    pub fn answer_approval(&self, approval_id: i64, approved: bool, scope: ApprovalScope) -> bool {
        self.inner
            .shared
            .answer_approval(approval_id, ApprovalAnswer { approved, scope })
    }

    pub fn approve(&self, approval_id: i64, scope: ApprovalScope) -> bool {
        self.answer_approval(approval_id, true, scope)
    }

    pub fn deny(&self, approval_id: i64) -> bool {
        self.answer_approval(approval_id, false, ApprovalScope::Once)
    }

    /// Cooperative stop: the run's loops see it, and both parked prompts are released.
    pub fn stop(&self) {
        self.inner.shared.stop();
    }

    /// Blocks until no turn is running; false when `timeout` expired first.
    ///
    /// Deterministic tests wait on this instead of sleeping: [`Session::send`] marks the run busy
    /// before it returns, so a wait that starts after `send` cannot miss the end of that turn.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.inner.shared.lock();
        while state.busy {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            if remaining.is_zero() {
                return false;
            }
            let (guard, result) = self
                .inner
                .shared
                .finished
                .wait_timeout(state, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = guard;
            if result.timed_out() && state.busy {
                return false;
            }
        }
        true
    }

    // --- Settings -----------------------------------------------------------------------------

    pub fn settings(&self) -> AppSettings {
        self.inner
            .settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .current()
            .clone()
    }

    /// Writes both preference files; the next turn uses the new endpoints.
    pub fn save_settings(&self, settings: AppSettings) -> Result<(), SessionError> {
        let secrets = settings.secrets();
        {
            let mut store = self
                .inner
                .settings
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            store
                .save(settings)
                .map_err(|error| SessionError::io("保存设置失败", error))?;
        }
        *self
            .inner
            .secrets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = secrets;
        Ok(())
    }

    // --- MCP ----------------------------------------------------------------------------------

    pub fn mcp_config(&self) -> McpSelection {
        self.inner
            .mcp
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .current()
            .clone()
    }

    pub fn save_mcp(&self, selection: McpSelection) -> Result<(), SessionError> {
        let mut store = self
            .inner
            .mcp
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        store
            .save(selection)
            .map_err(|error| SessionError::io("保存外部服务失败", error))
    }

    /// Connects to every server of `selection` once and reports what each one answered.
    ///
    /// Uses [`McpClient::initialize`] (implicitly, through [`McpClient::list_tools`]) on a real
    /// transport; nothing is cached afterwards.
    pub fn test_mcp(&self, selection: &McpSelection) -> Vec<ServerStatus> {
        let config = McpCatalog::built_in().to_config(selection, self.inner.platform);
        if config.servers.is_empty() {
            return Vec::new();
        }
        self.blocking(async move {
            let mut statuses = Vec::new();
            for server in config.servers {
                let status = match xiaoman_mcp::transport::create(&server) {
                    Ok(transport) => {
                        let client = McpClient::new(server.clone(), transport);
                        let result = client.list_tools().await;
                        client.close();
                        match result {
                            Ok(tools) => ServerStatus {
                                name: server.name.clone(),
                                title: server.title.clone(),
                                connected: true,
                                tool_count: tools.len(),
                                error: None,
                            },
                            Err(error) => ServerStatus {
                                name: server.name.clone(),
                                title: server.title.clone(),
                                connected: false,
                                tool_count: 0,
                                error: Some(error.to_string()),
                            },
                        }
                    }
                    Err(error) => ServerStatus {
                        name: server.name.clone(),
                        title: server.title.clone(),
                        connected: false,
                        tool_count: 0,
                        error: Some(error.to_string()),
                    },
                };
                statuses.push(status);
            }
            statuses
        })
    }

    // --- Memory and skills --------------------------------------------------------------------

    pub fn memory_list(&self, folder: Option<&str>) -> Result<Vec<Note>, MemoryError> {
        let memory = self.inner.memory.clone();
        self.blocking(async move { memory.list(folder).await })
    }

    pub fn memory_search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, MemoryError> {
        let memory = self.inner.memory.clone();
        let query = query.to_string();
        self.blocking(async move { memory.search(&query, limit).await })
    }

    pub fn memory_read(&self, identifier: &str) -> Result<Option<Note>, MemoryError> {
        let memory = self.inner.memory.clone();
        let identifier = identifier.to_string();
        self.blocking(async move { memory.read(&identifier).await })
    }

    /// Deletes the note's file and its index entries; false when nothing was there.
    pub fn memory_delete(&self, identifier: &str) -> Result<bool, MemoryError> {
        let memory = self.inner.memory.clone();
        let identifier = identifier.to_string();
        self.blocking(async move { memory.delete(&identifier).await })
    }

    pub fn skills_list(&self) -> Vec<Skill> {
        let skills = self.inner.skills.clone();
        self.blocking(async move { skills.list().await })
    }

    pub fn skills_reload(&self) -> SkillLoadResult {
        let skills = self.inner.skills.clone();
        self.blocking(async move { skills.reload().await })
    }

    /// The reminders the agent set during this session.
    pub fn scheduled_reminders(&self) -> Vec<(String, i64)> {
        self.inner.reminders.scheduled()
    }

    pub fn trace(&self) -> &Arc<TraceWriter> {
        &self.inner.trace
    }

    // --- Internals ----------------------------------------------------------------------------

    /// The agent for the current settings and MCP selection, rebuilt when either changed.
    fn agent(&self) -> Option<Arc<XiaomanAgent>> {
        let settings = self.settings();
        if !settings.configured() {
            return None;
        }
        let selection = self.mcp_config();
        let key = (settings.clone(), selection.clone());
        {
            let cached = self
                .inner
                .agent
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(cached) = cached.as_ref() {
                if cached.key == key {
                    return Some(cached.agent.clone());
                }
            }
        }

        let chat = (self.inner.llm)(&settings.chat.to_model_config(ModelRole::Chat));
        let operator = (self.inner.llm)(&settings.operator.to_model_config(ModelRole::Operator));
        let provider = self.mcp_provider(&selection);
        let users: Arc<dyn UserChannel> = self.inner.channels.clone();
        let device: Arc<dyn Device> = {
            let raw = self
                .inner
                .device
                .clone()
                .unwrap_or_else(|| Arc::new(DesktopDevice));
            Arc::new(RecordingDevice::new(raw, self.inner.trace.clone()))
        };
        let phone_tasks = self.inner.device.is_some();
        let persona = match &self.inner.persona {
            Some(persona) => persona.clone(),
            None if phone_tasks => xiaoman_agent::prompts::DEFAULT_PERSONA.to_string(),
            None => NO_PHONE_PERSONA.to_string(),
        };
        let agent = XiaomanAgent::new(
            AgentDeps {
                chat,
                operator,
                device,
                memory: self.inner.memory.clone(),
                skills: self.inner.skills.clone(),
                sentinel: self.inner.sentinel.clone(),
                users,
                reminders: self.inner.reminders.clone(),
            },
            AgentConfig {
                phone_tasks,
                ..settings.agent_config()
            },
        )
        .with_persona(persona)
        .with_id_factory({
            let run_ids = Arc::clone(&self.inner.run_ids);
            Arc::new(move || {
                run_ids
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .next_id()
            })
        });
        let agent = match &provider {
            Some(provider) => agent.with_external_tools(provider.clone()),
            None => agent,
        };
        let agent = Arc::new(agent);
        let mut cached = self
            .inner
            .agent
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(previous) = cached.replace(CachedAgent {
            key,
            agent: agent.clone(),
            provider,
        }) {
            if let Some(provider) = previous.provider {
                provider.close();
            }
        }
        Some(agent)
    }

    fn mcp_provider(&self, selection: &McpSelection) -> Option<Arc<McpToolProvider>> {
        let mut config = McpCatalog::built_in().to_config(selection, self.inner.platform);
        merge_file_servers(&mut config, &self.inner.data_dir);
        if config.servers.is_empty() {
            return None;
        }
        let users: Arc<dyn UserChannel> = self.inner.channels.clone();
        Some(Arc::new(McpToolProvider::new(
            config,
            Arc::new(UserChannelApprovals::new(users)),
        )))
    }

    fn spawn_turn(&self, agent: Arc<XiaomanAgent>, message: String) {
        let shared = self.inner.shared.clone();
        let listener: Arc<dyn AgentListener> = self.inner.channels.clone();
        let stop: Arc<dyn StopSignal> = self.inner.channels.clone();
        self.inner.runtime().spawn(async move {
            // Also runs on a panic, so the session never stays busy forever.
            let _guard = FinishGuard { shared };
            agent.handle(&message, listener, stop).await;
        });
    }

    fn spawn_phone_task(&self, agent: Arc<XiaomanAgent>, goal: String, skill: Option<String>) {
        let shared = self.inner.shared.clone();
        let listener: Arc<dyn AgentListener> = self.inner.channels.clone();
        let stop: Arc<dyn StopSignal> = self.inner.channels.clone();
        self.inner.runtime().spawn(async move {
            let _guard = FinishGuard { shared };
            agent
                .run_phone_task(&goal, skill.as_deref(), listener, stop)
                .await;
        });
    }

    /// Runs `future` on the session's runtime, from a thread that has no runtime of its own.
    ///
    /// The store accessors are local file I/O; a caller inside another runtime gets a scoped thread
    /// instead of a nested `block_on`, which tokio refuses.
    fn blocking<F, T>(&self, future: F) -> T
    where
        F: Future<Output = T> + Send,
        T: Send,
    {
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| {
                scope
                    .spawn(|| self.inner.runtime().block_on(future))
                    .join()
                    .expect("session worker panicked")
            })
        } else {
            self.inner.runtime().block_on(future)
        }
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("data_dir", &self.inner.data_dir)
            .field("busy", &self.is_busy())
            .finish_non_exhaustive()
    }
}

struct FinishGuard {
    shared: Arc<Shared>,
}

impl Drop for FinishGuard {
    fn drop(&mut self) {
        self.shared.finish();
    }
}

fn create_dir(dir: &Path) -> Result<(), SessionError> {
    std::fs::create_dir_all(dir)
        .map_err(|error| SessionError::io(&dir.display().to_string(), error))
}

/// The bundled `skills/examples` of this checkout, when the crate runs from a source tree.
fn repo_skill_examples() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("skills")
        .join("examples");
    root.is_dir().then_some(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// `mcp.json` carries the UI selection, but a hand-written `mcpServers` block in the same file
    /// is honoured too (the Kotlin `AppGraph.mcpConfig` behaviour).
    #[test]
    fn mcp_json_servers_are_merged_once() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut file = std::fs::File::create(dir.path().join("mcp.json")).expect("write mcp.json");
        write!(
            file,
            r#"{{"mcpServers":{{"handwritten":{{"url":"https://example.com/mcp","enabled":true}},"off":{{"url":"https://example.com/off","enabled":false}}}}}}"#
        )
        .expect("json");

        let mut config =
            McpCatalog::built_in().to_config(&McpSelection::default(), McpPlatform::Android);
        let catalog_servers = config.servers.len();
        merge_file_servers(&mut config, dir.path());
        assert_eq!(
            config.servers.len(),
            catalog_servers + 1,
            "only the enabled server is added"
        );
        assert!(
            config
                .servers
                .iter()
                .any(|server| server.name == "handwritten")
        );

        merge_file_servers(&mut config, dir.path());
        assert_eq!(
            config.servers.len(),
            catalog_servers + 1,
            "the second pass does not duplicate it"
        );
    }
}
