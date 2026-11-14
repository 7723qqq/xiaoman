//! `trace.jsonl`, `audit.jsonl` and `step-<n>.png`.
//!
//! Every trace line has exactly the keys `time, run, kind, step, action, text, ok, decision`, in
//! that order, with absent values written as JSON null (see `docs/ARCHITECTURE.md`, Traces).
//! Lines live in `<root>/<run_id>/trace.jsonl`; Sentinel decisions go to `audit.jsonl` beside
//! them and screenshots to `step-<n>.png`. Every event is also handed to the log sink as
//! `<run_id> <EventKind> <json>` (logcat in the app).
//!
//! The run id is the event's task id. Known secrets are masked in every line as a last line of
//! defence; events never carry them.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Map, Value};
use xiaoman_api::{
    Action, ActionResult, AgentEvent, AgentListener, AppInfo, AuditRecord, AuditSink, Decision,
    Device, DeviceError, ScreenState,
};

/// One line per agent event.
pub const TRACE_FILE: &str = "trace.jsonl";
/// One line per Sentinel decision.
pub const AUDIT_FILE: &str = "audit.jsonl";

/// Secrets shorter than this are not masked: a two-character key would mangle every line.
const MIN_SECRET_LEN: usize = 4;

type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;
type Secrets = Arc<dyn Fn() -> Vec<String> + Send + Sync>;
type Sink = Arc<dyn Fn(&str) + Send + Sync>;

// ---------------------------------------------------------------------------------------------
// Format
// ---------------------------------------------------------------------------------------------

/// Trace kind of `event`: the Kotlin class name, shared with [`AgentEvent::kind`].
pub fn kind(event: &AgentEvent) -> &'static str {
    event.kind()
}

/// `allow|ask|deny: <reason>`.
pub fn decision_text(decision: &Decision) -> String {
    let verdict = match decision {
        Decision::Allow { .. } => "allow",
        Decision::Ask { .. } => "ask",
        Decision::Deny { .. } => "deny",
    };
    format!("{verdict}: {}", decision.reason())
}

/// The action as the wire object, `{"type": "tap", ...}`.
pub fn action(action: &Action) -> Value {
    serde_json::to_value(action).expect("Action is always serializable")
}

/// One trace line. `executed_decision` is the Sentinel decision recorded for a `StepExecuted`
/// step, which the event itself does not carry.
pub fn line(event: &AgentEvent, time_millis: i64, executed_decision: Option<&str>) -> Value {
    let mut step: Option<i32> = None;
    let mut action_value: Option<Value> = None;
    let mut text: Option<String> = None;
    let mut ok: Option<bool> = None;
    let mut decision: Option<String> = None;
    match event {
        AgentEvent::Reply { text: reply, .. } => text = Some(reply.clone()),
        AgentEvent::SkillChosen { skill_name, .. } => text = Some(skill_name.clone()),
        AgentEvent::PhoneTaskStarted { goal, .. } => text = Some(goal.clone()),
        AgentEvent::StepPlanned {
            step_index,
            action: planned,
            ..
        } => {
            step = Some(*step_index);
            action_value = Some(action(planned));
        }
        AgentEvent::StepExecuted {
            step_index,
            action: executed,
            result,
            ..
        } => {
            step = Some(*step_index);
            action_value = Some(action(executed));
            ok = Some(result.ok);
            text = result.message.clone();
            decision = executed_decision.map(str::to_string);
        }
        AgentEvent::StepBlocked {
            step_index,
            action: blocked,
            decision: verdict,
            ..
        } => {
            step = Some(*step_index);
            action_value = Some(action(blocked));
            ok = Some(false);
            decision = Some(decision_text(verdict));
        }
        AgentEvent::NeedsUser { question, .. } => text = Some(question.clone()),
        AgentEvent::MemoryUpdated { permalink, .. } => text = Some(permalink.clone()),
        AgentEvent::ToolCalled {
            tool,
            summary,
            ok: success,
            ..
        } => {
            text = Some(format!("{tool}: {summary}"));
            ok = Some(*success);
        }
        AgentEvent::PhoneTaskFinished {
            success,
            summary,
            steps,
            ..
        } => {
            step = Some(*steps);
            text = Some(summary.clone());
            ok = Some(*success);
        }
        AgentEvent::Failed { message, .. } => text = Some(message.clone()),
    }
    let mut object = Map::new();
    object.insert("time".to_string(), Value::from(time_millis));
    object.insert("run".to_string(), Value::from(event.task_id()));
    object.insert("kind".to_string(), Value::from(kind(event)));
    object.insert("step".to_string(), step.map_or(Value::Null, Value::from));
    object.insert("action".to_string(), action_value.unwrap_or(Value::Null));
    object.insert("text".to_string(), text.map_or(Value::Null, Value::from));
    object.insert("ok".to_string(), ok.map_or(Value::Null, Value::from));
    object.insert(
        "decision".to_string(),
        decision.map_or(Value::Null, Value::from),
    );
    Value::Object(object)
}

/// One audit line: `time, run, step, action, decision, approved`.
pub fn audit(record: &AuditRecord) -> Value {
    let mut object = Map::new();
    object.insert("time".to_string(), Value::from(record.time_millis));
    object.insert("run".to_string(), Value::from(record.task_id.clone()));
    object.insert("step".to_string(), Value::from(record.step_index));
    object.insert("action".to_string(), action(&record.action));
    object.insert(
        "decision".to_string(),
        Value::from(decision_text(&record.decision)),
    );
    object.insert(
        "approved".to_string(),
        record.approved.map_or(Value::Null, Value::from),
    );
    Value::Object(object)
}

// ---------------------------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct State {
    /// run -> (step, decision) of the latest audit record, joined onto StepExecuted lines.
    last_decision: HashMap<String, (i32, String)>,
    current_run: Option<String>,
    current_step: i32,
}

/// Writes the trace, the audit log and the screenshots of every run below `root`.
///
/// Thread-safe: the state and the file appends share one lock, so concurrent events never
/// interleave inside a line. I/O failures go to the warning sink instead of stopping the task.
pub struct TraceWriter {
    root: PathBuf,
    clock: Clock,
    secrets: Secrets,
    log: Sink,
    warn: Sink,
    state: Mutex<State>,
}

/// A [`Device`] that hands every observed screenshot to the trace's `step-<n>.png`.
///
/// The screenshot of the
/// frame the operator saw is written beside `trace.jsonl`, named by the current step.
pub struct RecordingDevice {
    delegate: Arc<dyn Device>,
    trace: Arc<TraceWriter>,
}

impl RecordingDevice {
    pub fn new(delegate: Arc<dyn Device>, trace: Arc<TraceWriter>) -> Self {
        Self { delegate, trace }
    }
}

#[async_trait::async_trait]
impl Device for RecordingDevice {
    async fn observe(&self) -> Result<ScreenState, DeviceError> {
        let screen = self.delegate.observe().await?;
        if let Some(png) = &screen.screenshot_png {
            self.trace.save_screenshot(png);
        }
        Ok(screen)
    }

    async fn perform(&self, action: Action) -> ActionResult {
        self.delegate.perform(action).await
    }

    async fn installed_apps(&self) -> Vec<AppInfo> {
        self.delegate.installed_apps().await
    }
}

impl TraceWriter {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            clock: Arc::new(xiaoman_api::now_millis),
            secrets: Arc::new(Vec::new),
            log: Arc::new(|_| {}),
            warn: Arc::new(|_| {}),
            state: Mutex::new(State::default()),
        }
    }

    pub fn with_clock(self, clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        Self {
            clock: Arc::new(clock),
            ..self
        }
    }

    /// The configured API keys, masked in every line.
    pub fn with_secrets(self, secrets: impl Fn() -> Vec<String> + Send + Sync + 'static) -> Self {
        Self {
            secrets: Arc::new(secrets),
            ..self
        }
    }

    /// Receives `<run_id> <EventKind> <json>` for every event.
    pub fn with_log(self, log: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            log: Arc::new(log),
            ..self
        }
    }

    /// Receives one line per failed write or screenshot.
    pub fn with_warn(self, warn: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            warn: Arc::new(warn),
            ..self
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn run_dir(&self, run: &str) -> PathBuf {
        self.root.join(Self::safe_name(run))
    }

    /// Saves `png` as `step-<n>.png` of the running phone task, `n` being the next step index.
    pub fn save_screenshot(&self, png: &[u8]) {
        let (run, step) = {
            let state = self.lock();
            match &state.current_run {
                Some(run) => (run.clone(), state.current_step),
                None => return,
            }
        };
        let dir = self.run_dir(&run);
        let name = format!("step-{step}.png");
        let written =
            std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(&name), png));
        if let Err(error) = written {
            (self.warn)(&format!("trace: cannot save screenshot of {run}: {error}"));
        }
    }

    /// Run ids become folder names; anything that could escape the traces folder is replaced.
    pub fn safe_name(run: &str) -> String {
        let replaced: String = run
            .chars()
            .map(|value| match value {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '_' | '-' => value,
                _ => '_',
            })
            .collect();
        let trimmed = replaced.trim_matches('.');
        if trimmed.is_empty() {
            "_".to_string()
        } else {
            trimmed.to_string()
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic on another thread must not stop the trace of this one.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn mask(&self, line: &str) -> String {
        let mut out = line.to_string();
        for secret in (self.secrets)() {
            if secret.chars().count() >= MIN_SECRET_LEN {
                out = out.replace(secret.as_str(), "***");
            }
        }
        out
    }

    fn append(&self, run: &str, name: &str, line: &str) {
        let written = {
            let _guard = self.lock();
            write_line(&self.run_dir(run), name, line)
        };
        if let Err(error) = written {
            (self.warn)(&format!("trace: cannot write {name} of {run}: {error}"));
        }
    }
}

impl AgentListener for TraceWriter {
    fn on_event(&self, event: &AgentEvent) {
        let executed_decision = {
            let mut state = self.lock();
            track(&mut state, event)
        };
        let text =
            self.mask(&line(event, (self.clock)(), executed_decision.as_deref()).to_string());
        (self.log)(&format!("{} {} {}", event.task_id(), kind(event), text));
        self.append(event.task_id(), TRACE_FILE, &text);
    }
}

impl AuditSink for TraceWriter {
    fn record(&self, record: AuditRecord) {
        {
            let mut state = self.lock();
            state.last_decision.insert(
                record.task_id.clone(),
                (record.step_index, decision_text(&record.decision)),
            );
        }
        self.append(
            &record.task_id,
            AUDIT_FILE,
            &self.mask(&audit(&record).to_string()),
        );
    }
}

impl std::fmt::Debug for TraceWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceWriter")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

/// Must hold the lock. Returns the decision to put on a StepExecuted line.
fn track(state: &mut State, event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::PhoneTaskStarted { task_id, .. } => {
            state.current_run = Some(task_id.clone());
            state.current_step = 0;
            None
        }
        AgentEvent::StepExecuted {
            task_id,
            step_index,
            ..
        } => {
            if state.current_run.as_deref() == Some(task_id.as_str()) {
                state.current_step = step_index + 1;
            }
            state
                .last_decision
                .get(task_id)
                .filter(|(step, _)| step == step_index)
                .map(|(_, decision)| decision.clone())
        }
        AgentEvent::PhoneTaskFinished { task_id, .. } => {
            if state.current_run.as_deref() == Some(task_id.as_str()) {
                state.current_run = None;
            }
            state.last_decision.remove(task_id);
            None
        }
        _ => None,
    }
}

fn write_line(dir: &Path, name: &str, line: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(name))?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")
}
