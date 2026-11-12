//! The JNI implementation of [`crate::bridge`], built for Android only.
//!
//! Two hosts:
//!
//! * [`JniAccessibilityHost`] wraps the connected `XiaomanAccessibilityService` as a global
//!   reference and calls the methods of [`SERVICE_METHODS`] on it.
//! * [`JniForegroundHost`] calls the static methods of [`FOREGROUND_SERVICE_METHODS`] on
//!   `dev.xiaoman.app.AgentService`, passing the application context as a global reference.
//!
//! Every call attaches the calling thread, converts the result, and clears and describes a Java
//! exception the call left behind: failures are values (`false`, `0`, `null`), never panics.
//!
//! # What has been verified, and what has not
//!
//! The module is behind `#[cfg(target_os = "android")]` and its `jni` dependency lives in
//! `[target.'cfg(target_os = "android")'.dependencies]`, so the desktop workspace never builds it.
//!
//! Verified on an Android 15 x86_64 emulator against the Kotlin shell in `android/`:
//! `isConnected`, `displayWidthPx`/`displayHeightPx` and `JniForegroundHost::from_context`
//! (`FindClass` for `dev.xiaoman.app.AgentService` on the `onServiceConnected` thread) all round
//! trip, and `nativeOnForeground` reads both strings. The parts still to exercise on a device:
//!
//! * the `dev.xiaoman.hands.ScreenShot` accessors: this module assumes the Kotlin class exposes
//!   `getPng(): ByteArray?` and `getSecure(): boolean` (the usual Kotlin property getters). A data
//!   class with public fields, or different names, needs this file adjusted.
//! * `captureScreen`, `dispatchStroke` and the node table: the calls are written against
//!   [`SERVICE_METHODS`] but nothing has driven them through a real screen yet.
//! * a wrong declaration still surfaces as a cleared `NoSuchMethodError` and an all-false result,
//!   not as a compile error — check `SERVICE_METHODS` when a call silently returns `0`/`false`.

use jni::objects::{GlobalRef, JByteArray, JLongArray, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};

use crate::app_links::AlarmLink;
use crate::bridge::{
    AccessibilityHost, FOREGROUND_SERVICE, FOREGROUND_SERVICE_METHODS, ForegroundApp,
    ForegroundHost, JniMethod, NodeHandle, SERVICE_METHODS, ScreenShot,
};
use crate::geometry::Stroke;

/// The [`SERVICE_METHODS`] entry `name`, so a call site can never drift from the written contract.
fn service_method(name: &str) -> JniMethod {
    method(SERVICE_METHODS, name)
}

fn foreground_method(name: &str) -> JniMethod {
    method(FOREGROUND_SERVICE_METHODS, name)
}

fn method(table: &'static [JniMethod], name: &str) -> JniMethod {
    *table
        .iter()
        .find(|method| method.name == name)
        .unwrap_or_else(|| panic!("{name} is not in the JNI method table"))
}

/// Attaches the calling thread, runs `call` and clears a Java exception the call left behind.
/// An attach failure and a thrown exception both fall back to `R::default()`: failures are values.
fn with_env<R: Default>(vm: &JavaVM, call: impl FnOnce(&mut JNIEnv<'_>) -> R) -> R {
    let Ok(mut guard) = vm.attach_current_thread() else {
        return R::default();
    };
    let result = call(&mut guard);
    if guard.exception_check().unwrap_or(false) {
        // Logs the throwable and its stack trace, then restores a clean environment.
        let _ = guard.exception_describe();
        let _ = guard.exception_clear();
    }
    result
}

/// Reads a `java.lang.String`, or `None` for Java `null`.
fn java_string(env: &mut JNIEnv<'_>, object: JObject<'_>) -> Option<String> {
    if object.as_raw().is_null() {
        return None;
    }
    let string = JString::from(object);
    env.get_string(&string).ok().map(String::from)
}

/// [`AccessibilityHost`] over the connected `XiaomanAccessibilityService`.
pub struct JniAccessibilityHost {
    vm: JavaVM,
    service: GlobalRef,
}

impl JniAccessibilityHost {
    /// `service` is the object `nativeOnServiceConnected` handed Rust; it stays alive until the
    /// service is unbound.
    pub fn new(vm: JavaVM, service: GlobalRef) -> Self {
        Self { vm, service }
    }

    pub fn vm(&self) -> &JavaVM {
        &self.vm
    }

    fn call_bool(&self, name: &str) -> bool {
        let method = service_method(name);
        with_env(&self.vm, |env| {
            env.call_method(&self.service, method.name, method.signature, &[])
                .and_then(|value| value.z())
                .unwrap_or(false)
        })
    }

    fn call_bool_with(&self, name: &str, args: &[JValue]) -> bool {
        let method = service_method(name);
        with_env(&self.vm, |env| {
            env.call_method(&self.service, method.name, method.signature, args)
                .and_then(|value| value.z())
                .unwrap_or(false)
        })
    }

    fn call_int(&self, name: &str) -> i32 {
        let method = service_method(name);
        with_env(&self.vm, |env| {
            env.call_method(&self.service, method.name, method.signature, &[])
                .and_then(|value| value.i())
                .unwrap_or_default()
        })
    }

    fn call_int_with(&self, name: &str, args: &[JValue]) -> i32 {
        let method = service_method(name);
        with_env(&self.vm, |env| {
            env.call_method(&self.service, method.name, method.signature, args)
                .and_then(|value| value.i())
                .unwrap_or_default()
        })
    }

    fn call_long(&self, name: &str, args: &[JValue]) -> i64 {
        let method = service_method(name);
        with_env(&self.vm, |env| {
            env.call_method(&self.service, method.name, method.signature, args)
                .and_then(|value| value.j())
                .unwrap_or_default()
        })
    }

    fn call_string(&self, name: &str) -> Option<String> {
        self.call_string_with(name, &[])
    }

    fn call_string_with(&self, name: &str, args: &[JValue]) -> Option<String> {
        let method = service_method(name);
        with_env(&self.vm, |env| {
            let value = env
                .call_method(&self.service, method.name, method.signature, args)
                .ok()?;
            java_string(env, value.l().ok()?)
        })
    }

    fn call_unit(&self, name: &str, args: &[JValue]) {
        let method = service_method(name);
        with_env(&self.vm, |env| {
            let _ = env.call_method(&self.service, method.name, method.signature, args);
        });
    }
}

impl AccessibilityHost for JniAccessibilityHost {
    fn is_connected(&self) -> bool {
        self.call_bool("isConnected")
    }

    fn display_size(&self) -> (i32, i32) {
        (
            self.call_int("displayWidthPx").max(0),
            self.call_int("displayHeightPx").max(0),
        )
    }

    fn foreground(&self) -> Option<ForegroundApp> {
        let package_name = self.call_string("foregroundPackage")?;
        if package_name.is_empty() {
            return None;
        }
        Some(ForegroundApp {
            package_name,
            activity_name: self.call_string("foregroundActivity"),
        })
    }

    fn capture_screen(&self) -> ScreenShot {
        let method = service_method("captureScreen");
        let captured = with_env(&self.vm, |env| {
            let value = env
                .call_method(&self.service, method.name, method.signature, &[])
                .ok()?;
            let object = value.l().ok()?;
            if object.as_raw().is_null() {
                return None;
            }
            let png = env
                .call_method(&object, "getPng", "()[B", &[])
                .ok()
                .and_then(|value| value.l().ok())
                .filter(|png| !png.as_raw().is_null())
                .and_then(|png| env.convert_byte_array(&JByteArray::from(png)).ok());
            let secure = env
                .call_method(&object, "getSecure", "()Z", &[])
                .and_then(|value| value.z())
                .unwrap_or(false);
            Some(ScreenShot { png, secure })
        });
        captured.unwrap_or(ScreenShot {
            png: None,
            secure: false,
        })
    }

    fn roots(&self) -> Vec<NodeHandle> {
        let method = service_method("readRoots");
        let handles = with_env(&self.vm, |env| {
            let value = env
                .call_method(&self.service, method.name, method.signature, &[])
                .ok()?;
            let object = value.l().ok()?;
            if object.as_raw().is_null() {
                return None;
            }
            let array = JLongArray::from(object);
            let length = env.get_array_length(&array).ok()?;
            let mut raw = vec![0_i64; length.max(0) as usize];
            env.get_long_array_region(&array, 0, &mut raw).ok()?;
            Some(
                raw.into_iter()
                    .filter(|handle| *handle != 0)
                    .map(NodeHandle)
                    .collect(),
            )
        });
        handles.unwrap_or_default()
    }

    fn facts_json(&self, node: NodeHandle) -> Option<String> {
        self.call_string_with("nodeFactsJson", &[JValue::Long(node.0)])
    }

    fn child_count(&self, node: NodeHandle) -> usize {
        let count = self.call_int_with("nodeChildCount", &[JValue::Long(node.0)]);
        usize::try_from(count).unwrap_or_default()
    }

    fn child(&self, node: NodeHandle, index: usize) -> Option<NodeHandle> {
        let index = i32::try_from(index).ok()?;
        let handle = self.call_long("nodeChild", &[JValue::Long(node.0), JValue::Int(index)]);
        (handle != 0).then_some(NodeHandle(handle))
    }

    fn release(&self, node: NodeHandle) {
        self.call_unit("nodeRelease", &[JValue::Long(node.0)]);
    }

    fn refresh(&self, node: NodeHandle) -> bool {
        self.call_bool_with("nodeRefresh", &[JValue::Long(node.0)])
    }

    fn click(&self, node: NodeHandle) -> bool {
        self.call_bool_with("nodeClick", &[JValue::Long(node.0)])
    }

    fn long_click(&self, node: NodeHandle) -> bool {
        self.call_bool_with("nodeLongClick", &[JValue::Long(node.0)])
    }

    fn set_text(&self, node: NodeHandle, text: &str) -> bool {
        let method = service_method("nodeSetText");
        with_env(&self.vm, |env| {
            let Ok(text) = env.new_string(text) else {
                return false;
            };
            let text = JObject::from(text);
            env.call_method(
                &self.service,
                method.name,
                method.signature,
                &[JValue::Long(node.0), JValue::Object(&text)],
            )
            .and_then(|value| value.z())
            .unwrap_or(false)
        })
    }

    fn ime_enter(&self, node: NodeHandle) -> bool {
        self.call_bool_with("nodeImeEnter", &[JValue::Long(node.0)])
    }

    fn editable_focus(&self) -> Option<NodeHandle> {
        let handle = self.call_long("editableFocus", &[]);
        (handle != 0).then_some(NodeHandle(handle))
    }

    fn dispatch_stroke(&self, stroke: &Stroke) -> bool {
        let method = service_method("dispatchStroke");
        let mut points = Vec::with_capacity(stroke.points.len() * 2);
        for (x, y) in &stroke.points {
            points.push(*x);
            points.push(*y);
        }
        with_env(&self.vm, |env| {
            let Ok(array) = env.new_float_array(points.len() as i32) else {
                return false;
            };
            if env.set_float_array_region(&array, 0, &points).is_err() {
                return false;
            }
            let array = JObject::from(array);
            env.call_method(
                &self.service,
                method.name,
                method.signature,
                &[JValue::Object(&array), JValue::Long(stroke.duration_ms)],
            )
            .and_then(|value| value.z())
            .unwrap_or(false)
        })
    }

    fn global_back(&self) -> bool {
        self.call_bool("globalBack")
    }

    fn global_home(&self) -> bool {
        self.call_bool("globalHome")
    }

    fn launch(&self, package_name: &str) -> bool {
        let method = service_method("launch");
        with_env(&self.vm, |env| {
            let Ok(name) = env.new_string(package_name) else {
                return false;
            };
            let name = JObject::from(name);
            env.call_method(
                &self.service,
                method.name,
                method.signature,
                &[JValue::Object(&name)],
            )
            .and_then(|value| value.z())
            .unwrap_or(false)
        })
    }

    fn open_uri(&self, uri: &str, package_name: Option<&str>) -> bool {
        let method = service_method("openUri");
        with_env(&self.vm, |env| {
            let Ok(uri) = env.new_string(uri) else {
                return false;
            };
            let uri = JObject::from(uri);
            let package = match package_name {
                Some(name) => match env.new_string(name) {
                    Ok(name) => JObject::from(name),
                    Err(_) => return false,
                },
                None => JObject::null(),
            };
            env.call_method(
                &self.service,
                method.name,
                method.signature,
                &[JValue::Object(&uri), JValue::Object(&package)],
            )
            .and_then(|value| value.z())
            .unwrap_or(false)
        })
    }

    fn start_alarm(&self, link: &AlarmLink) -> bool {
        let method = service_method("startAlarm");
        with_env(&self.vm, |env| {
            let label = match link.label.as_deref() {
                Some(label) => match env.new_string(label) {
                    Ok(label) => JObject::from(label),
                    Err(_) => return false,
                },
                None => JObject::null(),
            };
            env.call_method(
                &self.service,
                method.name,
                method.signature,
                &[
                    JValue::Int(link.hour),
                    JValue::Int(link.minute),
                    JValue::Object(&label),
                ],
            )
            .and_then(|value| value.z())
            .unwrap_or(false)
        })
    }

    fn installed_apps_json(&self) -> String {
        self.call_string("installedAppsJson").unwrap_or_default()
    }
}

/// [`ForegroundHost`] over the static methods of `dev.xiaoman.app.AgentService`.
pub struct JniForegroundHost {
    vm: JavaVM,
    context: GlobalRef,
    class: GlobalRef,
}

impl JniForegroundHost {
    /// `context` is the application context the static methods take; `class` is a global reference
    /// to `dev.xiaoman.app.AgentService`.
    pub fn new(vm: JavaVM, context: GlobalRef, class: GlobalRef) -> Self {
        Self { vm, context, class }
    }

    /// Resolves the service class and keeps it as a global reference. Call it on a JVM-owned
    /// thread: `FindClass` from an attached native thread uses the system class loader.
    pub fn from_context(vm: JavaVM, context: GlobalRef) -> Option<Self> {
        let name = FOREGROUND_SERVICE.replace('.', "/");
        let class = {
            let mut guard = vm.attach_current_thread().ok()?;
            let class = guard.find_class(&name).ok()?;
            guard.new_global_ref(class).ok()?
        };
        Some(Self { vm, context, class })
    }

    pub fn vm(&self) -> &JavaVM {
        &self.vm
    }
}

impl ForegroundHost for JniForegroundHost {
    fn start(&self) -> bool {
        let method = foreground_method("start");
        with_env(&self.vm, |env| {
            env.call_static_method(
                &self.class,
                method.name,
                method.signature,
                &[JValue::Object(self.context.as_obj())],
            )
            .and_then(|value| value.z())
            .unwrap_or(false)
        })
    }

    fn is_running(&self) -> bool {
        let method = foreground_method("isRunning");
        with_env(&self.vm, |env| {
            env.call_static_method(&self.class, method.name, method.signature, &[])
                .and_then(|value| value.z())
                .unwrap_or(false)
        })
    }

    fn set_status(&self, status: &str) {
        let method = foreground_method("updateStatus");
        with_env(&self.vm, |env| {
            let Ok(status) = env.new_string(status) else {
                return;
            };
            let status = JObject::from(status);
            let _ = env.call_static_method(
                &self.class,
                method.name,
                method.signature,
                &[
                    JValue::Object(self.context.as_obj()),
                    JValue::Object(&status),
                ],
            );
        });
    }

    fn stop(&self) {
        let method = foreground_method("stop");
        with_env(&self.vm, |env| {
            let _ = env.call_static_method(
                &self.class,
                method.name,
                method.signature,
                &[JValue::Object(self.context.as_obj())],
            );
        });
    }
}
