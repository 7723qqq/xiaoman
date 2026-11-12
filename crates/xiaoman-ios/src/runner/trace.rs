//! One run's trace: `step-NNN.png` per observed screen and `events.jsonl` with screens, actions,
//! results, audit records and agent events. Known API keys are replaced by `***`; text typed into
//! password fields is never written.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::TimeZone;
use serde_json::{Map, Value};
use xiaoman_api::{Action, AgentEvent, AuditRecord, AuditSink, Device, DeviceError, ScreenState};

type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Writes the trace of one run below `<traces>/<yyyyMMdd-HHmmss>-<id>/`.
pub struct TraceRecorder {
    dir: PathBuf,
    secrets: Vec<String>,
    clock: Clock,
    step: AtomicI32,
    last_screen: Mutex<Option<ScreenState>>,
    write_lock: Mutex<()>,
}

impl TraceRecorder {
    pub fn new(traces_root: impl AsRef<Path>, secrets: Vec<String>) -> Self {
        Self::with_clock(traces_root, secrets, Arc::new(xiaoman_api::now_millis))
    }

    pub fn with_clock(traces_root: impl AsRef<Path>, secrets: Vec<String>, clock: Clock) -> Self {
        let now = (clock)();
        let name = format!("{}-{}", timestamp(now), suffix(now));
        Self {
            dir: traces_root.as_ref().join(name),
            secrets,
            clock,
            step: AtomicI32::new(0),
            last_screen: Mutex::new(None),
            write_lock: Mutex::new(()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Wraps `delegate` so every observe and perform lands in the trace.
    pub fn device(self: &Arc<Self>, delegate: Arc<dyn Device>) -> Arc<dyn Device> {
        Arc::new(TracedDevice {
            trace: self.clone(),
            delegate,
        })
    }

    pub fn event(&self, event: &AgentEvent) {
        let detail = match event {
            AgentEvent::StepPlanned { action, .. } => self.describe(action),
            AgentEvent::StepExecuted { action, result, .. } => {
                format!("{} -> {result:?}", self.describe(action))
            }
            AgentEvent::StepBlocked {
                action, decision, ..
            } => {
                format!("{} -> {decision:?}", self.describe(action))
            }
            other => format!("{other:?}"),
        };
        self.write(
            "event",
            vec![
                ("kind", Value::from(event.kind())),
                ("task", Value::from(event.task_id())),
                ("detail", Value::from(detail)),
            ],
        );
    }

    /// Action text for the trace, hiding what is typed into a password field.
    pub fn describe(&self, action: &Action) -> String {
        if self.types_into_password(action) {
            if let Action::TypeText {
                node_id,
                submit,
                target,
                reason,
                ..
            } = action
            {
                let masked = Action::TypeText {
                    text: "***".to_string(),
                    node_id: *node_id,
                    submit: *submit,
                    target: target.clone(),
                    reason: reason.clone(),
                };
                return format!("{masked:?}");
            }
        }
        format!("{action:?}")
    }

    fn types_into_password(&self, action: &Action) -> bool {
        let Action::TypeText { node_id, .. } = action else {
            return false;
        };
        let screen = self
            .last_screen
            .lock()
            .ok()
            .and_then(|screen| screen.clone());
        let Some(screen) = screen else { return false };
        let target = match node_id {
            Some(id) => screen.nodes.iter().find(|node| node.id == *id),
            None => screen
                .nodes
                .iter()
                .find(|node| node.focused && node.editable),
        };
        target.is_some_and(|node| node.password)
    }

    fn write(&self, kind: &str, fields: Vec<(&str, Value)>) {
        let mut line = Map::new();
        line.insert("time".to_string(), Value::from((self.clock)()));
        line.insert("type".to_string(), Value::from(kind));
        for (name, value) in fields {
            line.insert(name.to_string(), value);
        }
        let text = self.redact(&Value::Object(line).to_string());
        let _guard = self.write_lock.lock();
        let _ = std::fs::create_dir_all(&self.dir);
        let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("events.jsonl"))
        else {
            return;
        };
        let _ = writeln!(file, "{text}");
    }

    fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            if secret.trim().is_empty() {
                continue;
            }
            out = out.replace(secret.as_str(), "***");
        }
        out
    }
}

impl AuditSink for TraceRecorder {
    fn record(&self, record: AuditRecord) {
        let mut fields = vec![
            ("task", Value::from(record.task_id)),
            ("step", Value::from(record.step_index)),
            ("action", Value::from(self.describe(&record.action))),
            ("decision", Value::from(format!("{:?}", record.decision))),
        ];
        if let Some(approved) = record.approved {
            fields.push(("approved", Value::from(approved)));
        }
        self.write("audit", fields);
    }
}

struct TracedDevice {
    trace: Arc<TraceRecorder>,
    delegate: Arc<dyn Device>,
}

#[async_trait]
impl Device for TracedDevice {
    async fn observe(&self) -> Result<ScreenState, DeviceError> {
        let screen = self
            .delegate
            .observe()
            .await
            .map_err(|error| DeviceError(error.to_string()))?;
        if let Ok(mut last) = self.trace.last_screen.lock() {
            *last = Some(screen.clone());
        }
        let step = self.trace.step.fetch_add(1, Ordering::SeqCst) + 1;
        let screenshot = screen.screenshot_png.as_ref().and_then(|png| {
            let name = format!("step-{step:03}.png");
            std::fs::create_dir_all(&self.trace.dir).ok()?;
            std::fs::write(self.trace.dir.join(&name), png).ok()?;
            Some(name)
        });
        let mut fields = vec![
            ("step", Value::from(step)),
            ("package", value_or_null(screen.package_name.as_deref())),
            (
                "size",
                Value::from(format!("{}x{}", screen.width_px, screen.height_px)),
            ),
            ("nodes", Value::from(screen.nodes.len() as i64)),
        ];
        if let Some(screenshot) = screenshot {
            fields.push(("screenshot", Value::from(screenshot)));
        }
        self.trace.write("observe", fields);
        Ok(screen)
    }

    async fn perform(&self, action: Action) -> xiaoman_api::ActionResult {
        let result = self.delegate.perform(action.clone()).await;
        let mut fields = vec![
            ("action", Value::from(self.trace.describe(&action))),
            ("ok", Value::from(result.ok)),
        ];
        if let Some(message) = &result.message {
            fields.push(("message", Value::from(message.clone())));
        }
        self.trace.write("perform", fields);
        result
    }

    async fn installed_apps(&self) -> Vec<xiaoman_api::AppInfo> {
        self.delegate.installed_apps().await
    }
}

fn value_or_null(value: Option<&str>) -> Value {
    match value {
        Some(value) => Value::from(value),
        None => Value::Null,
    }
}

fn timestamp(millis: i64) -> String {
    match chrono::Local.timestamp_millis_opt(millis).single() {
        Some(time) => time.format("%Y%m%d-%H%M%S").to_string(),
        None => millis.to_string(),
    }
}

fn suffix(millis: i64) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or_default();
    let mixed = (millis as u64)
        .wrapping_mul(1_000_000_007)
        .wrapping_add(u64::from(nanos));
    format!("{:06x}", mixed & 0xff_ffff)
}
