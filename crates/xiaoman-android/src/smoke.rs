//! The scripted "see and touch" run: the parts of the JNI seam no test has driven on a device.
//!
//! `adb shell am broadcast -a dev.xiaoman.smoke` reaches [`run_smoke`] through
//! `dev.xiaoman.app.SmokeReceiver` and the `runSmoke` native of `dev.xiaoman.hands.Smoke`. The run
//! drives [`AndroidDevice`] over the live [`crate::jni_exports::service_host`] with no model in the
//! loop, so every step goes through the real path:
//!
//! * `readRoots`/`nodeFactsJson`/`nodeRelease` and the handle ownership of
//!   [`AndroidDevice`]'s observation;
//! * `captureScreen`, including the `FLAG_SECURE` / all-black answer;
//! * `dispatchStroke` and its `GestureResultCallback` wait, through a tap on a node;
//! * `Back` and `Home`, and the four `AgentService` statics of the foreground host.
//!
//! The script is deliberately tolerant: a missing Settings app, a different layout or a missing
//! label is logged with what was actually on screen and the run continues. Nothing panics, and the
//! summary string says what happened rather than what was expected.
//!
//! Every line carries the `XiaomanSmoke` tag, so `adb logcat -s XiaomanSmoke` is the whole output.

use std::ffi::{CString, c_char};
use std::sync::Arc;
use std::time::Instant;

use xiaoman_api::{Action, Device, ScreenState, UiNode};

use crate::bridge::AccessibilityHost;
use crate::device::AndroidDevice;

// ---------------------------------------------------------------------------- logcat

/// The tag of every line this module writes.
const LOG_TAG: &[u8] = b"XiaomanSmoke\0";

const ANDROID_LOG_INFO: i32 = 4;
const ANDROID_LOG_WARN: i32 = 5;

#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_write(priority: i32, tag: *const c_char, text: *const c_char) -> i32;
}

/// One logcat line. A message with an interior NUL is dropped rather than truncated.
fn log(priority: i32, message: &str) {
    let Ok(text) = CString::new(message) else {
        return;
    };
    // Safety: both pointers are NUL-terminated and outlive the call.
    unsafe { __android_log_write(priority, LOG_TAG.as_ptr() as *const c_char, text.as_ptr()) };
}

// ---------------------------------------------------------------------------- script

/// The app the run opens: Settings exists on every Android image.
const SETTINGS_PACKAGE: &str = "com.android.settings";

/// The Settings entries the run taps, in the order they are tried: the Display label first, then
/// entries of the pages the Settings launcher intent resumes. This emulator restores the last
/// Settings page (`网络和互联网`) instead of the top-level list that holds `显示`.
const SETTINGS_LABELS: &[&str] = &[
    "显示",
    "Display",
    "互联网",
    "Internet",
    "已连接的设备",
    "Connected devices",
    "应用",
    "Apps",
    "通知",
    "Notifications",
    "电池",
    "Battery",
    "存储",
    "Storage",
];

/// How many node texts a negative result dumps.
const NODE_TEXT_PREVIEW: usize = 10;

/// How many node texts the post-tap observation prints.
const AFTER_TAP_PREVIEW: usize = 5;

/// How long the foreground service gets to come up before the run stops waiting.
const FOREGROUND_START_TIMEOUT_MS: u64 = 2_000;

/// How long the service gets between `onCreate` and `startForeground` before `stop` is safe.
const FOREGROUND_SETTLE_MS: u64 = 600;

/// How long the foreground service gets to go down.
const FOREGROUND_STOP_TIMEOUT_MS: u64 = 2_000;

/// Interval of the `isRunning` polls.
const FOREGROUND_POLL_MS: u64 = 50;

/// Runs the scripted pass and returns the one-line summary Kotlin logs.
///
/// Blocking: it captures screenshots, waits out Android's screenshot interval and waits for gesture
/// callbacks, so it must not run on the main thread. `SmokeReceiver` calls it on a plain thread.
pub fn run_smoke() -> String {
    let Some(host) = crate::jni_exports::service_host() else {
        log(
            ANDROID_LOG_WARN,
            "run: the accessibility service is not connected; enable it and retry",
        );
        return "smoke skipped: the accessibility service is not connected".to_string();
    };
    if let Some(foreground) = crate::jni_exports::foreground_host() {
        drive_foreground_host(foreground.as_ref());
    } else {
        log(
            ANDROID_LOG_WARN,
            "run: no AgentService host; its four statics stay unproven",
        );
    }

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let message = format!("run: cannot build a runtime: {error}");
            log(ANDROID_LOG_WARN, &message);
            return format!("smoke skipped: {message}");
        }
    };
    runtime.block_on(script(host))
}

/// The four `FOREGROUND_SERVICE_METHODS` in one round trip.
///
/// The order matters: `start` hands a pending `startForegroundService` to the platform, and the
/// service only calls `startForeground` later, in `onStartCommand`. Stopping before that lands makes
/// Android throw `ForegroundServiceDidNotStartInTimeException` at the app's main thread and kill the
/// process, so the run waits for the service to be alive and settles before `stop`.
fn drive_foreground_host(host: &dyn crate::bridge::ForegroundHost) {
    let started = host.start();
    let running = wait_for(|| host.is_running(), FOREGROUND_START_TIMEOUT_MS);
    if running {
        host.set_status("smoke: see and touch");
    }
    // `startForeground` sits behind `onCreate` in the service's own message queue.
    std::thread::sleep(std::time::Duration::from_millis(FOREGROUND_SETTLE_MS));
    host.stop();
    let stopped = wait_for(|| !host.is_running(), FOREGROUND_STOP_TIMEOUT_MS);
    log(
        ANDROID_LOG_INFO,
        &format!(
            "0 foreground host: start={started} running={running} updateStatus called, \
             stop called, stopped={stopped}"
        ),
    );
}

/// Polls `probe` every [`FOREGROUND_POLL_MS`] until it is true or `timeout_ms` elapses.
fn wait_for(probe: impl Fn() -> bool, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if probe() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(FOREGROUND_POLL_MS));
    }
    probe()
}

async fn script(host: Arc<dyn AccessibilityHost>) -> String {
    let mut run = Run {
        device: AndroidDevice::new(host),
        summary: Vec::new(),
        steps: 0,
        failures: 0,
    };

    let Some(before) = run.observe("1 observe").await else {
        run.note("no first observation");
        return run.finish();
    };
    run.note(format!("start={}", foreground(&before)));

    let launched = run
        .perform(
            "2 launch",
            Action::LaunchApp {
                package_name: SETTINGS_PACKAGE.to_string(),
                target: Some("显示".to_string()),
                reason: Some("smoke: open the Settings app".to_string()),
            },
        )
        .await;
    run.note(format!("launch={}", word(launched)));

    let after_launch = run.observe("3 observe after launch").await;
    match &after_launch {
        Some(state) => {
            run.note(format!("launched={}", foreground(state)));
            if state.package_name.as_deref() != Some(SETTINGS_PACKAGE) {
                log(
                    ANDROID_LOG_WARN,
                    &format!(
                        "3 observe after launch: {} is not {SETTINGS_PACKAGE}; first \
                         {NODE_TEXT_PREVIEW} node texts: {}",
                        foreground(state),
                        node_texts(state, NODE_TEXT_PREVIEW)
                    ),
                );
            }
        }
        None => run.note("launched=<no observation>"),
    }

    let target = after_launch
        .as_ref()
        .and_then(|state| find_node(state, SETTINGS_LABELS));
    let mut tapped = false;
    match target {
        Some(node) => {
            let id = node.id;
            let label = node_label(node);
            run.note(format!("tap=node#{id} \"{label}\""));
            tapped = run
                .perform(
                    "4 tap",
                    Action::Tap {
                        point: None,
                        node_id: Some(id),
                        target: Some(label),
                        reason: Some("smoke: open the Display page".to_string()),
                    },
                )
                .await;
        }
        None => {
            run.failures += 1;
            run.note("tap=no node matched");
            if let Some(state) = &after_launch {
                log(
                    ANDROID_LOG_WARN,
                    &format!(
                        "4 tap: no node whose text or description contains {SETTINGS_LABELS:?}; \
                         first {NODE_TEXT_PREVIEW} node texts: {}",
                        node_texts(state, NODE_TEXT_PREVIEW)
                    ),
                );
            }
        }
    }

    if tapped {
        let previous = after_launch.as_ref().map(ScreenState::fingerprint);
        if let Some(state) = run.observe("5 observe after tap").await {
            let changed = previous.is_none_or(|fingerprint| fingerprint != state.fingerprint());
            run.note(format!("after={} changed={changed}", foreground(&state)));
            log(
                ANDROID_LOG_INFO,
                &format!(
                    "5 observe after tap: package={} activity={} nodes={} texts={}",
                    optional(state.package_name.as_deref(), "<none>"),
                    optional(state.activity_name.as_deref(), "<none>"),
                    state.nodes.len(),
                    node_texts(&state, AFTER_TAP_PREVIEW)
                ),
            );
        }
    }

    let back = run
        .perform(
            "6 back",
            Action::Back {
                target: None,
                reason: Some("smoke: check GLOBAL_ACTION_BACK".to_string()),
            },
        )
        .await;
    let home = run
        .perform(
            "7 home",
            Action::Home {
                target: None,
                reason: Some("smoke: check GLOBAL_ACTION_HOME".to_string()),
            },
        )
        .await;
    run.note(format!("back={} home={}", word(back), word(home)));

    if let Some(state) = run.observe("8 observe after back and home").await {
        run.note(format!(
            "final={} nodes={}",
            foreground(&state),
            state.nodes.len()
        ));
    }
    run.finish()
}

// ---------------------------------------------------------------------------- the run

/// One script: the device, the segments of the summary and the failure count.
struct Run {
    device: AndroidDevice,
    summary: Vec<String>,
    steps: u32,
    failures: u32,
}

impl Run {
    fn note(&mut self, segment: impl Into<String>) {
        self.summary.push(segment.into());
    }

    /// One observation, logging the foreground app, the node count, the secure flag and the PNG
    /// size. `None` when the device could not observe; the run continues with what it has.
    async fn observe(&mut self, label: &str) -> Option<ScreenState> {
        self.steps += 1;
        let started = Instant::now();
        match self.device.observe().await {
            Ok(state) => {
                let bytes = state.screenshot_png.as_ref().map_or(0, Vec::len);
                log(
                    ANDROID_LOG_INFO,
                    &format!(
                        "{label}: package={} activity={} nodes={} secure={} png={bytes}B \
                         fingerprint={} in {}ms",
                        optional(state.package_name.as_deref(), "<none>"),
                        optional(state.activity_name.as_deref(), "<none>"),
                        state.nodes.len(),
                        state.secure,
                        state.fingerprint(),
                        started.elapsed().as_millis()
                    ),
                );
                Some(state)
            }
            Err(error) => {
                self.failures += 1;
                log(
                    ANDROID_LOG_WARN,
                    &format!(
                        "{label}: observe failed: {error} in {}ms",
                        started.elapsed().as_millis()
                    ),
                );
                None
            }
        }
    }

    /// One action and its timing. A refused action counts as a failure but never stops the run.
    async fn perform(&mut self, label: &str, action: Action) -> bool {
        self.steps += 1;
        let started = Instant::now();
        let result = self.device.perform(action).await;
        if !result.ok {
            self.failures += 1;
        }
        log(
            if result.ok {
                ANDROID_LOG_INFO
            } else {
                ANDROID_LOG_WARN
            },
            &format!(
                "{label}: ok={} message={} in {}ms",
                result.ok,
                result.message.as_deref().unwrap_or("-"),
                started.elapsed().as_millis()
            ),
        );
        result.ok
    }

    /// The summary Kotlin logs, and the return value of `runSmoke`.
    fn finish(self) -> String {
        let status = if self.failures == 0 {
            "ok"
        } else {
            "incomplete"
        };
        let summary = format!(
            "smoke {status}: {} steps, {} failed; {}",
            self.steps,
            self.failures,
            self.summary.join("; ")
        );
        log(ANDROID_LOG_INFO, &summary);
        summary
    }
}

// ---------------------------------------------------------------------------- helpers

fn word(ok: bool) -> &'static str {
    if ok { "ok" } else { "failed" }
}

fn optional<'a>(value: Option<&'a str>, fallback: &'a str) -> &'a str {
    value.unwrap_or(fallback)
}

/// `package/activity`, or the package alone when the window state event named no activity.
fn foreground(state: &ScreenState) -> String {
    let package = optional(state.package_name.as_deref(), "<none>");
    match state.activity_name.as_deref() {
        Some(activity) => format!("{package}/{activity}"),
        None => package.to_string(),
    }
}

/// The text a tap is reported by: the node's text, else its description.
fn node_label(node: &UiNode) -> String {
    match node.text.as_deref().filter(|text| !text.trim().is_empty()) {
        Some(text) => text.to_string(),
        None => node.description.as_deref().unwrap_or_default().to_string(),
    }
}

/// The first `limit` non-empty node texts, for a negative result.
fn node_texts(state: &ScreenState, limit: usize) -> String {
    let texts: Vec<&str> = state
        .nodes
        .iter()
        .filter_map(|node| {
            node.text
                .as_deref()
                .filter(|text| !text.trim().is_empty())
                .or_else(|| {
                    node.description
                        .as_deref()
                        .filter(|text| !text.trim().is_empty())
                })
        })
        .take(limit)
        .collect();
    if texts.is_empty() {
        "<no text>".to_string()
    } else {
        texts.join(" | ")
    }
}

/// The first node whose text or description is one of `needles`, in needle order.
///
/// An exact label wins over a substring: on `网络和互联网` both the page title and the `互联网`
/// entry contain the word, and tapping the title would dispatch a gesture that changes nothing.
fn find_node<'a>(state: &'a ScreenState, needles: &[&str]) -> Option<&'a UiNode> {
    needles
        .iter()
        .find_map(|needle| {
            state
                .nodes
                .iter()
                .find(|node| node_values(node).any(|value| value == *needle))
        })
        .or_else(|| {
            needles.iter().find_map(|needle| {
                state
                    .nodes
                    .iter()
                    .find(|node| node_values(node).any(|value| value.contains(needle)))
            })
        })
}

/// The text and the description of a node, the two fields the run looks labels up in.
fn node_values(node: &UiNode) -> impl Iterator<Item = &str> {
    [node.text.as_deref(), node.description.as_deref()]
        .into_iter()
        .flatten()
}
