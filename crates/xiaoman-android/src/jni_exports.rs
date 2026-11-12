//! The `Java_dev_xiaoman_*` entry points: the four rows of [`crate::bridge::SERVICE_NATIVES`] and
//! the one row of [`crate::bridge::FOREGROUND_SERVICE_NATIVES`], exported from
//! `libxiaoman_android.so`.
//!
//! All five are **instance** natives, so the second JNI argument is the object the method ran on
//! (`jobject`), never the class (`jclass`). `AgentService.nativeRequestStop` is the one the
//! README calls out: it is the foreground service instance, not its companion object, even though
//! the four `FOREGROUND_SERVICE_METHODS` Rust calls are static members of that companion.
//!
//! # What the exports own
//!
//! `nativeOnServiceConnected` is the only place a [`JavaVM`] and a `GlobalRef` to the live service
//! can come from, so it also builds the two hosts of [`crate::jni_bridge`] and keeps them for the
//! rest of the process:
//!
//! * [`service_host`] — the [`crate::bridge::AccessibilityHost`] the device calls.
//! * [`foreground_host`] — the [`crate::bridge::ForegroundHost`] over `AgentService`'s statics. It
//!   is resolved there because `FindClass` on a native thread sees the system class loader, not
//!   the app's; `onServiceConnected` runs on a JVM-owned thread.
//!
//! [`stop_signal`] and the last foreground app survive an unbind: the operator loop still has to
//! see the Stop control after the user unplugged the service.
//!
//! # Failures
//!
//! Nothing here panics or throws into the JVM. A failed `GlobalRef`, a missing VM or an unreadable
//! string is logged to logcat under the `XiaomanRust` tag and the export returns.

use std::ffi::{CString, c_char};
use std::sync::{Arc, Mutex, OnceLock};

use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{JObject, JString};
use jni::sys::JavaVM as RawJavaVM;

use crate::bridge::{
    AccessibilityHost, FOREGROUND_SERVICE_NATIVES, ForegroundApp, JniMethod, SERVICE_NATIVES,
    SMOKE_NATIVES,
};
use crate::jni_bridge::{JniAccessibilityHost, JniForegroundHost};
use crate::stop_signal::MutableStopSignal;

// ---------------------------------------------------------------------------- logcat

/// The tag every line of this module carries, next to the Kotlin `Xiaoman` tag.
const LOG_TAG: &[u8] = b"XiaomanRust\0";

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

// ---------------------------------------------------------------------------- state

/// A `JavaVM` handle as it can be kept in a `static`: the pointer the JVM owns for the life of the
/// process. Fresh handles are handed out by [`VmPtr::java_vm`]; `JavaVM` itself is not `Clone`.
#[derive(Clone, Copy)]
struct VmPtr(*mut RawJavaVM);

// Safety: the pointer is the process-wide VM, valid from the first native call to process exit.
unsafe impl Send for VmPtr {}
unsafe impl Sync for VmPtr {}

impl VmPtr {
    fn of(vm: &JavaVM) -> Self {
        Self(vm.get_java_vm_pointer())
    }

    fn java_vm(self) -> Option<JavaVM> {
        // Safety: `of` only ever stores the pointer `JNIEnv::get_java_vm` returned.
        unsafe { JavaVM::from_raw(self.0) }.ok()
    }
}

#[derive(Default)]
struct Bridge {
    vm: Option<VmPtr>,
    service: Option<Arc<JniAccessibilityHost>>,
    foreground_service: Option<Arc<JniForegroundHost>>,
    foreground: Option<ForegroundApp>,
}

static BRIDGE: OnceLock<Mutex<Bridge>> = OnceLock::new();
static STOP: OnceLock<MutableStopSignal> = OnceLock::new();

fn with_state<R>(block: impl FnOnce(&mut Bridge) -> R) -> R {
    let mut guard = BRIDGE
        .get_or_init(|| Mutex::new(Bridge::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    block(&mut guard)
}

/// The Stop control shared by the notification's action, `AgentService.nativeRequestStop` and the
/// operator loop. Set once and stays set until [`MutableStopSignal::reset`].
pub fn stop_signal() -> &'static MutableStopSignal {
    STOP.get_or_init(MutableStopSignal::new)
}

/// The connected accessibility service, or `None` before `onServiceConnected` and after
/// `onUnbind`. The device is disconnected exactly when this is `None`.
pub fn service_host() -> Option<Arc<JniAccessibilityHost>> {
    with_state(|state| state.service.clone())
}

/// The `AgentService` statics, resolved when the accessibility service connected.
pub fn foreground_host() -> Option<Arc<JniForegroundHost>> {
    with_state(|state| state.foreground_service.clone())
}

/// The last `TYPE_WINDOW_STATE_CHANGED` window, kept across an unbind.
pub fn foreground_app() -> Option<ForegroundApp> {
    with_state(|state| state.foreground.clone())
}

/// The VM, for a runtime thread of the app that has to attach itself.
pub fn java_vm() -> Option<JavaVM> {
    with_state(|state| state.vm.and_then(VmPtr::java_vm))
}

/// Debug assertion: every export names a row of the table it implements, so a rename on one side
/// cannot drift from the other unnoticed.
fn exported(table: &'static [JniMethod], name: &str) {
    debug_assert!(
        table.iter().any(|method| method.name == name),
        "{name} is not a row of its native table"
    );
}

/// Reads a `java.lang.String`, or `None` for Java `null`.
fn java_string(env: &mut JNIEnv<'_>, object: JObject<'_>) -> Option<String> {
    if object.as_raw().is_null() {
        return None;
    }
    env.get_string(&JString::from(object))
        .ok()
        .map(String::from)
}

// ---------------------------------------------------------------------------- SERVICE_NATIVES

/// `nativeOnServiceConnected()V` — the service is live: keep it and build the two hosts.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_hands_XiaomanAccessibilityService_nativeOnServiceConnected(
    env: JNIEnv<'_>,
    service: JObject<'_>,
) {
    exported(SERVICE_NATIVES, "nativeOnServiceConnected");
    if service.as_raw().is_null() {
        log(
            ANDROID_LOG_WARN,
            "nativeOnServiceConnected: null service object",
        );
        return;
    }
    let Ok(vm) = env.get_java_vm() else {
        log(
            ANDROID_LOG_WARN,
            "nativeOnServiceConnected: no JavaVM for this thread",
        );
        return;
    };
    let Ok(global) = env.new_global_ref(&service) else {
        log(
            ANDROID_LOG_WARN,
            "nativeOnServiceConnected: cannot keep a global ref to the service",
        );
        return;
    };
    let vm_handle = VmPtr::of(&vm);
    let host = Arc::new(JniAccessibilityHost::new(vm, global));

    // `FindClass` for `dev.xiaoman.app.AgentService` only resolves against the app's class loader
    // on a thread the JVM attached, which `onServiceConnected` is.
    let foreground_service = env
        .get_java_vm()
        .ok()
        .zip(env.new_global_ref(&service).ok())
        .and_then(|(vm, context)| JniForegroundHost::from_context(vm, context))
        .map(Arc::new);

    // One round trip back into Java proves the method table of `SERVICE_METHODS` resolves on this
    // build; a signature mismatch shows up here as `0x0` instead of the display size.
    let connected = host.is_connected();
    let (width, height) = host.display_size();
    let foreground = foreground_service.is_some();
    with_state(|state| {
        state.vm = Some(vm_handle);
        state.service = Some(host);
        state.foreground_service = foreground_service;
    });
    log(
        ANDROID_LOG_INFO,
        &format!(
            "nativeOnServiceConnected: connected={connected} display={width}x{height} \
             foreground-service={foreground}"
        ),
    );
}

/// `nativeOnServiceUnbound()V` — `onUnbind`/`onDestroy`: drop the service and its global refs.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_hands_XiaomanAccessibilityService_nativeOnServiceUnbound(
    _env: JNIEnv<'_>,
    _service: JObject<'_>,
) {
    exported(SERVICE_NATIVES, "nativeOnServiceUnbound");
    // Dropped outside the lock: `GlobalRef` deletes a JVM reference on drop.
    let (service, foreground_service) =
        with_state(|state| (state.service.take(), state.foreground_service.take()));
    let had = service.is_some();
    drop((service, foreground_service));
    log(
        ANDROID_LOG_INFO,
        &format!("nativeOnServiceUnbound: host dropped={had}; the last foreground app is kept"),
    );
}

/// `nativeRequestStop()V` — the Stop control: the cooperative flag of the running task.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_hands_XiaomanAccessibilityService_nativeRequestStop(
    _env: JNIEnv<'_>,
    _service: JObject<'_>,
) {
    exported(SERVICE_NATIVES, "nativeRequestStop");
    stop_signal().stop();
    log(ANDROID_LOG_INFO, "nativeRequestStop: stop signal set");
}

/// `nativeOnForeground(Ljava/lang/String;Ljava/lang/String;)V` — a `TYPE_WINDOW_STATE_CHANGED`
/// window. Both arguments may be null; a null package is an event the service does not count as a
/// foreground change.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_hands_XiaomanAccessibilityService_nativeOnForeground(
    mut env: JNIEnv<'_>,
    _service: JObject<'_>,
    package_name: JObject<'_>,
    activity_name: JObject<'_>,
) {
    exported(SERVICE_NATIVES, "nativeOnForeground");
    let Some(package_name) = java_string(&mut env, package_name) else {
        return;
    };
    if package_name.is_empty() {
        return;
    }
    let foreground = ForegroundApp {
        package_name: package_name.clone(),
        activity_name: java_string(&mut env, activity_name),
    };
    let activity = foreground.activity_name.clone().unwrap_or_default();
    with_state(|state| state.foreground = Some(foreground));
    log(
        ANDROID_LOG_INFO,
        &format!("nativeOnForeground: {package_name}/{activity}"),
    );
}

// ------------------------------------------------------------------- FOREGROUND_SERVICE_NATIVES

/// `nativeRequestStop()V` on `dev.xiaoman.app.AgentService` — the notification's Stop action.
///
/// The second argument is the service instance (`jobject`), not the class: the Kotlin declaration
/// is an instance `external fun`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_app_AgentService_nativeRequestStop(
    _env: JNIEnv<'_>,
    _service: JObject<'_>,
) {
    exported(FOREGROUND_SERVICE_NATIVES, "nativeRequestStop");
    stop_signal().stop();
    // The GPUI/Compose session has its own stop flag; the notification's Stop hits this export,
    // not `RustSession.nativeStop`, so release the run and its parked prompts here too.
    if let Some(session) = crate::session::current() {
        session.stop();
    }
    log(
        ANDROID_LOG_INFO,
        "AgentService.nativeRequestStop: stop signal set",
    );
}

// ---------------------------------------------------------------------------- SMOKE_NATIVES

/// `runSmoke()Ljava/lang/String;` on `dev.xiaoman.hands.Smoke` — the scripted see-and-touch run of
/// [`crate::smoke`], reached by `adb shell am broadcast -a dev.xiaoman.smoke`.
///
/// `dev.xiaoman.app.SmokeReceiver` calls it on a plain thread, never on the main thread: the run
/// captures screenshots and waits for gesture callbacks. The second argument is the `Smoke` object,
/// or the class when the declaration carries `@JvmStatic`; the symbol and descriptor are the same
/// either way, and the run does not need it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_xiaoman_hands_Smoke_runSmoke(
    env: JNIEnv<'_>,
    _this: JObject<'_>,
) -> jni::sys::jstring {
    exported(SMOKE_NATIVES, "runSmoke");
    let summary = crate::smoke::run_smoke();
    match env.new_string(summary) {
        Ok(text) => text.into_raw(),
        Err(error) => {
            log(
                ANDROID_LOG_WARN,
                &format!("runSmoke: cannot build the result string: {error}"),
            );
            std::ptr::null_mut()
        }
    }
}
