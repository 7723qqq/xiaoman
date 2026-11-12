//! The app-level JNI seam: one [`xiaoman_app::Session`] behind `dev.xiaoman.app.RustSession`.
//!
//! The accessibility bridge in [`crate::bridge`] gives Rust the phone; this module gives the
//! Kotlin shell the agent. One session per process, created by `HostApplication` with the app's
//! files directory and the extracted bundled-skill folder.
//!
//! The surface is kept small on purpose: state goes back as JSON ([`xiaoman_app::json`]) that the
//! Compose screens render, and every call is cheap enough for the main thread except the memory
//! and MCP ones, which Kotlin runs on `Dispatchers.IO`.
//!
//! Events are not pushed: the shell polls [`nativeSnapshot`] while a screen is visible. The
//! session already re-reads its whole transcript for a snapshot, so a dropped event can never
//! desynchronise the UI.

#![cfg(target_os = "android")]

use std::ffi::CString;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong, jstring};
use jni::{JNIEnv, JavaVM};
use serde_json::{Value, json};
use xiaoman_api::{Device, DeviceError, ReminderScheduler};
use xiaoman_app::json as view;
use xiaoman_app::session::{SendOutcome, Session, SessionConfig};
use xiaoman_app::settings::AppSettings;
use xiaoman_mcp::{McpCatalog, McpPlatform};

use crate::device::AndroidDevice;

const TAG: &[u8] = b"Xiaoman\0";

#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_write(
        priority: i32,
        tag: *const std::ffi::c_char,
        text: *const std::ffi::c_char,
    ) -> i32;
}

fn log(priority: i32, message: &str) {
    let Ok(text) = CString::new(message) else {
        return;
    };
    // Safety: both pointers are NUL-terminated and outlive the call.
    unsafe {
        __android_log_write(
            priority,
            TAG.as_ptr() as *const std::ffi::c_char,
            text.as_ptr(),
        )
    };
}

fn log_info(message: &str) {
    log(4, message);
}

fn log_warn(message: &str) {
    log(5, message);
}

// ---------------------------------------------------------------------------------------------
// Process-wide state
// ---------------------------------------------------------------------------------------------

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static VM: OnceLock<JavaVM> = OnceLock::new();
/// The Kotlin `NotifierBridge` object, passed in by `HostApplication` on a JVM thread.
static NOTIFIER: Mutex<Option<GlobalRef>> = Mutex::new(None);

fn with_session<T>(f: impl FnOnce(&Session) -> T) -> Option<T> {
    let guard = SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.as_ref().map(f)
}

fn session_or_null<T>(f: impl FnOnce(&Session) -> T) -> Option<T> {
    let found = with_session(f);
    if found.is_none() {
        log_warn("RustSession: no session; HostApplication did not open one");
    }
    found
}

// ---------------------------------------------------------------------------------------------
// Device: the accessibility host, resolved when the service is connected
// ---------------------------------------------------------------------------------------------

/// The phone's hands, resolved lazily so a session opened before the accessibility service binds
/// still works once the user switches it on.
#[derive(Default)]
struct ServiceDevice {
    cached: Mutex<
        Option<(
            Arc<dyn crate::bridge::AccessibilityHost>,
            Arc<AndroidDevice>,
        )>,
    >,
}

impl ServiceDevice {
    fn device(&self) -> Result<Arc<AndroidDevice>, DeviceError> {
        let host: Arc<dyn crate::bridge::AccessibilityHost> = crate::jni_exports::service_host()
            .ok_or_else(|| {
                DeviceError("无障碍服务未开启，请先在系统设置里打开小满的无障碍服务".to_string())
            })?;
        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((cached_host, device)) = cached.as_ref() {
            if Arc::ptr_eq(cached_host, &host) {
                return Ok(device.clone());
            }
        }
        let device = Arc::new(AndroidDevice::new(host.clone()));
        *cached = Some((host, device.clone()));
        Ok(device)
    }
}

#[async_trait::async_trait]
impl Device for ServiceDevice {
    async fn observe(&self) -> Result<xiaoman_api::ScreenState, DeviceError> {
        self.device()?.observe().await
    }

    async fn perform(&self, action: xiaoman_api::Action) -> xiaoman_api::ActionResult {
        match self.device() {
            Ok(device) => device.perform(action).await,
            Err(error) => xiaoman_api::ActionResult::failed(error.to_string()),
        }
    }

    async fn installed_apps(&self) -> Vec<xiaoman_api::AppInfo> {
        match self.device() {
            Ok(device) => device.installed_apps().await,
            Err(_) => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Reminders: AlarmManager through the Kotlin host
// ---------------------------------------------------------------------------------------------

/// Hands `reminder_set` to `dev.xiaoman.app.AlarmReminders`, which posts the alarm and the
/// notification. A missing VM or a Java exception only loses that one reminder.
struct JniReminders;

impl ReminderScheduler for JniReminders {
    fn schedule(&self, text: &str, at_millis: i64) {
        let Some(vm) = VM.get() else {
            log_warn("RustSession: reminder without a JVM");
            return;
        };
        let Ok(mut env) = vm.attach_current_thread() else {
            log_warn("RustSession: cannot attach the reminder thread");
            return;
        };
        let Ok(text) = env.new_string(text) else {
            return;
        };
        if let Err(error) = env.call_static_method(
            "dev/xiaoman/app/AlarmReminders",
            "schedule",
            "(Ljava/lang/String;J)V",
            &[JValue::Object(&text), JValue::Long(at_millis)],
        ) {
            log_warn(&format!(
                "RustSession: AlarmReminders.schedule failed: {error}"
            ));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// JNI helpers
// ---------------------------------------------------------------------------------------------

fn string_arg(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.as_raw().is_null() {
        return None;
    }
    env.get_string(value).ok().map(String::from)
}

fn optional_string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    string_arg(env, value).filter(|text| !text.trim().is_empty())
}

fn json_string(env: &mut JNIEnv, value: &Value) -> jstring {
    match env.new_string(value.to_string()) {
        Ok(text) => text.into_raw(),
        Err(error) => {
            log_warn(&format!("RustSession: cannot return JSON: {error}"));
            std::ptr::null_mut()
        }
    }
}

fn text_string(env: &mut JNIEnv, value: &str) -> jstring {
    match env.new_string(value) {
        Ok(text) => text.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn outcome_name(outcome: SendOutcome) -> &'static str {
    match outcome {
        SendOutcome::Answered => "answered",
        SendOutcome::Started => "started",
        SendOutcome::Busy => "busy",
        SendOutcome::NotConfigured => "notConfigured",
        SendOutcome::Empty => "empty",
    }
}

// ---------------------------------------------------------------------------------------------
// Exports: lifecycle and chat
// ---------------------------------------------------------------------------------------------

/// Opens the one session of this process.
///
/// Called by the Kotlin `HostApplication` through `nativeOpen`, and directly by the GPUI
/// `android_main`, so the Compose shell and the Rust UI share the exact same session.
pub fn open(data_dir: impl AsRef<Path>, bundled_skills: Option<&Path>) -> Result<(), String> {
    let data_dir = data_dir.as_ref();
    if current().is_some() {
        return Ok(());
    }
    if VM.get().is_none() {
        // `android-activity` publishes the process VM through `ndk-context`; wrapping it here
        // lets `JniReminders` call `AlarmReminders.schedule` from the UI thread as well.
        let context = ndk_context::android_context();
        let raw = context.vm().cast::<jni::sys::JavaVM>();
        if !raw.is_null() {
            if let Ok(vm) = unsafe { JavaVM::from_raw(raw) } {
                let _ = VM.set(vm);
            }
        }
    }
    let mut config = SessionConfig::new(data_dir)
        .with_platform(McpPlatform::Android)
        .with_device(Arc::new(ServiceDevice::default()))
        .with_reminders(Arc::new(JniReminders))
        // API keys go through the Android Keystore, never to `settings-secrets.json`.
        .with_settings_factory(crate::secret_store::factory())
        .with_log_sink(|line| log_info(line));
    if let Some(root) = bundled_skills {
        config = config.with_skill_root(root);
    }
    let session = Session::new(config).map_err(|error| error.to_string())?;
    let mut slot = SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *slot = Some(session);
    log_info(&format!(
        "RustSession: session opened under {}",
        data_dir.display()
    ));
    Ok(())
}

/// The open session, for the in-process GPUI UI.
pub fn current() -> Option<Session> {
    let guard = SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.clone()
}

/// Drops the session (tests and process shutdown).
pub fn close() {
    let mut slot = SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if slot.take().is_some() {
        log_info("RustSession: session closed");
    }
}

/// Calls a method on the Kotlin `NotifierBridge` object.
///
/// The object is kept as a global reference because `FindClass` on a Rust-created thread sees the
/// system class loader, not the app's; the object was handed over on a JVM-owned thread instead.
fn call_notifier(name: &str, signature: &str, args: &[JValue]) {
    let Some(vm) = VM.get() else {
        log_warn(&format!("RustSession: {name} without a JVM"));
        return;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        log_warn(&format!("RustSession: cannot attach for {name}"));
        return;
    };
    let guard = NOTIFIER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(notifier) = guard.as_ref() else {
        log_warn(&format!("RustSession: {name} without a NotifierBridge"));
        return;
    };
    if let Err(error) = env.call_method(notifier.as_obj(), name, signature, args) {
        log_warn(&format!(
            "RustSession: NotifierBridge.{name} failed: {error}"
        ));
        // A pending exception would poison the next JNI call on this thread.
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
    }
}

/// Starts the foreground service before a turn that may leave the Activity.
pub fn start_service() {
    call_notifier("startService", "()V", &[]);
}

pub fn stop_service() {
    call_notifier("stopService", "()V", &[]);
}

pub fn update_status(text: Option<&str>) {
    let Some(vm) = VM.get() else { return };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let value = match text {
        Some(text) => match env.new_string(text) {
            Ok(text) => JObject::from(text),
            Err(_) => return,
        },
        None => JObject::null(),
    };
    let guard = NOTIFIER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(notifier) = guard.as_ref() else {
        return;
    };
    if let Err(error) = env.call_method(
        notifier.as_obj(),
        "updateStatus",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&value)],
    ) {
        log_warn(&format!("RustSession: updateStatus call failed: {error}"));
    }
}

pub fn show_approval(id: i64, prompt: &str, reason: &str) {
    log_info(&format!(
        "RustSession: showApproval id={id} prompt={prompt}"
    ));
    let Some(vm) = VM.get() else { return };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let (Ok(prompt), Ok(reason)) = (env.new_string(prompt), env.new_string(reason)) else {
        return;
    };
    let guard = NOTIFIER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(notifier) = guard.as_ref() else {
        log_warn("RustSession: showApproval without a NotifierBridge");
        return;
    };
    if let Err(error) = env.call_method(
        notifier.as_obj(),
        "showApproval",
        "(JLjava/lang/String;Ljava/lang/String;)V",
        &[
            JValue::Long(id),
            JValue::Object(&prompt),
            JValue::Object(&reason),
        ],
    ) {
        log_warn(&format!("RustSession: showApproval call failed: {error}"));
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
    }
}

pub fn hide_approval() {
    call_notifier("hideApproval", "()V", &[]);
}

pub fn show_question(text: &str, takeover: bool) {
    let Some(vm) = VM.get() else { return };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let Ok(text) = env.new_string(text) else {
        return;
    };
    let guard = NOTIFIER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(notifier) = guard.as_ref() else {
        return;
    };
    if let Err(error) = env.call_method(
        notifier.as_obj(),
        "showQuestion",
        "(Ljava/lang/String;Z)V",
        &[JValue::Object(&text), JValue::Bool(u8::from(takeover))],
    ) {
        log_warn(&format!("RustSession: showQuestion call failed: {error}"));
    }
}

pub fn hide_question() {
    call_notifier("hideQuestion", "()V", &[]);
}

fn call_notifier_bool(name: &str, signature: &str) -> bool {
    let Some(vm) = VM.get() else { return false };
    let Ok(mut env) = vm.attach_current_thread() else {
        return false;
    };
    let guard = NOTIFIER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(notifier) = guard.as_ref() else {
        return false;
    };
    match env.call_method(notifier.as_obj(), name, signature, &[]) {
        Ok(value) => value.z().unwrap_or(false),
        Err(error) => {
            log_warn(&format!(
                "RustSession: NotifierBridge.{name} failed: {error}"
            ));
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            false
        }
    }
}

pub fn accessibility_enabled() -> bool {
    call_notifier_bool("accessibilityEnabled", "()Z")
}

pub fn notifications_allowed() -> bool {
    call_notifier_bool("notificationsAllowed", "()Z")
}

pub fn service_running() -> bool {
    call_notifier_bool("serviceRunning", "()Z")
}

pub fn library_loaded() -> bool {
    call_notifier_bool("libraryLoaded", "()Z")
}

pub fn open_accessibility_settings() {
    call_notifier("openAccessibilitySettings", "()V", &[]);
}

pub fn open_notification_settings() {
    call_notifier("openNotificationSettings", "()V", &[]);
}

/// The approval card's "see the app underneath" move: the host activity goes to the back before
/// the answer is delivered. False when no activity is on screen (a notification action, say).
pub fn hide_app() -> bool {
    call_notifier_bool("hideApp", "()Z")
}

/// POST_NOTIFICATIONS on Android 13+, from the chat banner or the first start.
pub fn request_notifications() {
    call_notifier("requestNotifications", "()V", &[]);
}

/// `ACTION_VIEW` for a catalog key's help page, e.g. the 去申请 link.
pub fn open_uri(uri: &str) {
    let Some(vm) = VM.get() else { return };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let Ok(url) = env.new_string(uri) else { return };
    let guard = NOTIFIER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(notifier) = guard.as_ref() else {
        return;
    };
    if let Err(error) = env.call_method(
        notifier.as_obj(),
        "openUri",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&url)],
    ) {
        log_warn(&format!("RustSession: openUri failed: {error}"));
    }
}

/// One decrypted value of the Keystore secret store, or `None` when the key is absent.
pub fn read_secret(name: &str) -> Option<String> {
    let vm = VM.get()?;
    let mut env = vm.attach_current_thread().ok()?;
    let key = env.new_string(name).ok()?;
    let guard = NOTIFIER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let notifier = guard.as_ref()?;
    let value = env.call_method(
        notifier.as_obj(),
        "readSecret",
        "(Ljava/lang/String;)Ljava/lang/String;",
        &[JValue::Object(&key)],
    );
    match value {
        Ok(value) => {
            let object = value.l().ok()?;
            if object.is_null() {
                return None;
            }
            let text: JString = object.into();
            let text = env.get_string(&text).ok()?;
            Some(text.to_string_lossy().into_owned())
        }
        Err(error) => {
            log_warn(&format!("RustSession: readSecret({name}) failed: {error}"));
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            None
        }
    }
}

/// Stores one value through the Keystore; false when the bridge is not there or refused.
pub fn write_secret(name: &str, value: &str) -> bool {
    let Some(vm) = VM.get() else { return false };
    let Ok(mut env) = vm.attach_current_thread() else {
        return false;
    };
    let (Ok(key), Ok(text)) = (env.new_string(name), env.new_string(value)) else {
        return false;
    };
    let guard = NOTIFIER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(notifier) = guard.as_ref() else {
        return false;
    };
    let called = env.call_method(
        notifier.as_obj(),
        "writeSecret",
        "(Ljava/lang/String;Ljava/lang/String;)Z",
        &[JValue::Object(&key), JValue::Object(&text)],
    );
    match called {
        Ok(value) => value.z().unwrap_or(false),
        Err(error) => {
            log_warn(&format!("RustSession: writeSecret({name}) failed: {error}"));
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_describe();
                let _ = env.exception_clear();
            }
            false
        }
    }
}

/// Drops one stored value, e.g. when a scope stops holding it.
pub fn remove_secret(name: &str) {
    let Some(vm) = VM.get() else { return };
    let Ok(mut env) = vm.attach_current_thread() else {
        return;
    };
    let Ok(key) = env.new_string(name) else {
        return;
    };
    let guard = NOTIFIER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(notifier) = guard.as_ref() else {
        return;
    };
    if let Err(error) = env.call_method(
        notifier.as_obj(),
        "removeSecret",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&key)],
    ) {
        log_warn(&format!(
            "RustSession: removeSecret({name}) failed: {error}"
        ));
    }
}

/// `nativeOpen(dataDir, bundledSkillsDir)`: opens the one session of this process.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeOpen(
    mut env: JNIEnv,
    _this: JObject,
    data_dir: JString,
    bundled_skills: JString,
    notifier: JObject,
) -> jboolean {
    let Some(data_dir) = string_arg(&mut env, &data_dir) else {
        log_warn("RustSession.nativeOpen: empty data directory");
        return JNI_FALSE;
    };
    if let Ok(vm) = env.get_java_vm() {
        let _ = VM.set(vm);
    }
    match env.new_global_ref(&notifier) {
        Ok(global) => {
            *NOTIFIER
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(global);
            log_info("RustSession: notifier stored");
        }
        Err(error) => log_warn(&format!("RustSession: notifier global ref failed: {error}")),
    }
    let bundled = optional_string(&mut env, &bundled_skills);
    match open(&data_dir, bundled.as_deref().map(Path::new)) {
        Ok(()) => JNI_TRUE,
        Err(error) => {
            log_warn(&format!("RustSession: cannot open the session: {error}"));
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeClose(_env: JNIEnv, _this: JObject) {
    close();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeSnapshot(
    mut env: JNIEnv,
    _this: JObject,
) -> jstring {
    let snapshot = session_or_null(|session| view::snapshot(&session.state()));
    match snapshot {
        Some(value) => json_string(&mut env, &value),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeSend(
    mut env: JNIEnv,
    _this: JObject,
    text: JString,
) -> jstring {
    let Some(text) = string_arg(&mut env, &text) else {
        return json_string(&mut env, &json!({"outcome": "empty"}));
    };
    let outcome = session_or_null(|session| session.send(&text));
    let value = match outcome {
        Some(outcome) => json!({"outcome": outcome_name(outcome)}),
        None => json!({"outcome": "notConfigured"}),
    };
    json_string(&mut env, &value)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeRunTask(
    mut env: JNIEnv,
    _this: JObject,
    goal: JString,
    skill: JString,
    run_id: JString,
) -> jboolean {
    let Some(goal) = string_arg(&mut env, &goal) else {
        return JNI_FALSE;
    };
    let skill = optional_string(&mut env, &skill);
    let run_id = optional_string(&mut env, &run_id);
    let started = session_or_null(|session| {
        session.run_phone_task(&goal, skill.as_deref(), run_id.as_deref())
    })
    .unwrap_or(false);
    if started { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeFail(
    mut env: JNIEnv,
    _this: JObject,
    run_id: JString,
    message: JString,
) {
    let Some(run_id) = string_arg(&mut env, &run_id) else {
        return;
    };
    let message = string_arg(&mut env, &message).unwrap_or_else(|| "任务无法开始".to_string());
    session_or_null(|session| session.fail(&run_id, &message));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeApprove(
    mut env: JNIEnv,
    _this: JObject,
    approval_id: jlong,
    scope: JString,
) -> jboolean {
    let scope = string_arg(&mut env, &scope).unwrap_or_default();
    let scope = if scope.eq_ignore_ascii_case("task") {
        xiaoman_api::ApprovalScope::Task
    } else {
        xiaoman_api::ApprovalScope::Once
    };
    let answered =
        session_or_null(|session| session.approve(approval_id as i64, scope)).unwrap_or(false);
    if answered { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeDeny(
    _env: JNIEnv,
    _this: JObject,
    approval_id: jlong,
) -> jboolean {
    let answered = session_or_null(|session| session.deny(approval_id as i64)).unwrap_or(false);
    if answered { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeAnswerQuestion(
    mut env: JNIEnv,
    _this: JObject,
    text: JString,
) -> jboolean {
    let Some(text) = string_arg(&mut env, &text) else {
        return JNI_FALSE;
    };
    let answered = session_or_null(|session| session.answer_question(&text)).unwrap_or(false);
    if answered { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeStop(_env: JNIEnv, _this: JObject) {
    session_or_null(Session::stop);
}

// ---------------------------------------------------------------------------------------------
// Exports: settings, skills, memory, MCP
// ---------------------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeSettings(
    mut env: JNIEnv,
    _this: JObject,
) -> jstring {
    match session_or_null(|session| view::settings(&session.settings())) {
        Some(value) => json_string(&mut env, &value),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeSaveSettings(
    mut env: JNIEnv,
    _this: JObject,
    value: JString,
) -> jboolean {
    let Some(text) = string_arg(&mut env, &value) else {
        return JNI_FALSE;
    };
    let parsed: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            log_warn(&format!("RustSession: settings JSON is invalid: {error}"));
            return JNI_FALSE;
        }
    };
    let settings: AppSettings = match view::settings_from_value(&parsed) {
        Ok(settings) => settings,
        Err(error) => {
            log_warn(&format!("RustSession: settings rejected: {error}"));
            return JNI_FALSE;
        }
    };
    let saved = session_or_null(|session| session.save_settings(settings).is_ok()).unwrap_or(false);
    if saved { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeSkills(
    mut env: JNIEnv,
    _this: JObject,
) -> jstring {
    match session_or_null(|session| {
        let skills = session.skills_list();
        Value::Array(skills.iter().map(view::skill).collect())
    }) {
        Some(value) => json_string(&mut env, &value),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeReloadSkills(
    mut env: JNIEnv,
    _this: JObject,
) -> jstring {
    match session_or_null(|session| view::skill_load(&session.skills_reload())) {
        Some(value) => json_string(&mut env, &value),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeMemoryList(
    mut env: JNIEnv,
    _this: JObject,
    folder: JString,
) -> jstring {
    let folder = optional_string(&mut env, &folder);
    match session_or_null(|session| session.memory_list(folder.as_deref())) {
        Some(Ok(notes)) => {
            let value = json!({"notes": notes.iter().map(view::note).collect::<Vec<_>>()});
            json_string(&mut env, &value)
        }
        Some(Err(error)) => json_string(&mut env, &view::result_error(error.to_string())),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeMemorySearch(
    mut env: JNIEnv,
    _this: JObject,
    query: JString,
    limit: jint,
) -> jstring {
    let Some(query) = string_arg(&mut env, &query) else {
        return std::ptr::null_mut();
    };
    let limit = limit.max(0) as usize;
    match session_or_null(|session| session.memory_search(&query, limit)) {
        Some(Ok(hits)) => json_string(&mut env, &json!({"hits": view::search_hits(&hits)})),
        Some(Err(error)) => json_string(&mut env, &view::result_error(error.to_string())),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeMemoryRead(
    mut env: JNIEnv,
    _this: JObject,
    identifier: JString,
) -> jstring {
    let Some(identifier) = string_arg(&mut env, &identifier) else {
        return std::ptr::null_mut();
    };
    match session_or_null(|session| session.memory_read(&identifier)) {
        Some(Ok(Some(note))) => json_string(&mut env, &json!({"note": view::note(&note)})),
        Some(Ok(None)) => json_string(&mut env, &json!({"note": Value::Null})),
        Some(Err(error)) => json_string(&mut env, &view::result_error(error.to_string())),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeMemoryDelete(
    mut env: JNIEnv,
    _this: JObject,
    identifier: JString,
) -> jboolean {
    let Some(identifier) = string_arg(&mut env, &identifier) else {
        return JNI_FALSE;
    };
    let deleted = session_or_null(|session| session.memory_delete(&identifier))
        .and_then(Result::ok)
        .unwrap_or(false);
    if deleted { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeMcp(
    mut env: JNIEnv,
    _this: JObject,
) -> jstring {
    match session_or_null(|session| {
        let selection = session.mcp_config();
        let catalog = McpCatalog::built_in();
        view::catalog(&catalog.servers, McpPlatform::Android, &selection)
    }) {
        Some(value) => json_string(&mut env, &value),
        None => std::ptr::null_mut(),
    }
}

fn selection_from(env: &mut JNIEnv, value: &JString) -> Option<xiaoman_mcp::McpSelection> {
    let text = string_arg(env, value)?;
    let parsed: Value = serde_json::from_str(&text).ok()?;
    match view::selection_from_value(&parsed) {
        Ok(selection) => Some(selection),
        Err(error) => {
            log_warn(&format!("RustSession: MCP selection rejected: {error}"));
            None
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeSaveMcp(
    mut env: JNIEnv,
    _this: JObject,
    value: JString,
) -> jboolean {
    let Some(selection) = selection_from(&mut env, &value) else {
        return JNI_FALSE;
    };
    let saved = session_or_null(|session| session.save_mcp(selection).is_ok()).unwrap_or(false);
    if saved { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeTestMcp(
    mut env: JNIEnv,
    _this: JObject,
    value: JString,
) -> jstring {
    let Some(selection) = selection_from(&mut env, &value) else {
        return json_string(&mut env, &view::result_error("外部服务配置无效"));
    };
    match session_or_null(|session| session.test_mcp(&selection)) {
        Some(statuses) => json_string(&mut env, &view::server_statuses(&statuses)),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeTracesDir(
    mut env: JNIEnv,
    _this: JObject,
) -> jstring {
    match session_or_null(|session| session.traces_dir().display().to_string()) {
        Some(path) => text_string(&mut env, &path),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_RustSession_nativeDataDir(
    mut env: JNIEnv,
    _this: JObject,
) -> jstring {
    match session_or_null(|session| session.data_dir().display().to_string()) {
        Some(path) => text_string(&mut env, &path),
        None => std::ptr::null_mut(),
    }
}
