//! The JNI seam to the two Java classes the Android build keeps.
//!
//! This module is the written contract and links no `jni`: both halves are written against the same
//! names. [`crate::jni_bridge`] implements [`AccessibilityHost`] and [`ForegroundHost`] over these
//! tables, and [`crate::jni_exports`] exports the `native` methods of [`SERVICE_NATIVES`] and
//! [`FOREGROUND_SERVICE_NATIVES`]. What follows lists the classes, the methods each one must
//! expose, the values that cross the boundary and the threading rules the implementation obeys.
//!
//! # The two classes
//!
//! | Class | Role |
//! | --- | --- |
//! | [`ACCESSIBILITY_SERVICE`] | The user-enabled `AccessibilityService`: the only object allowed to call accessibility APIs. It owns no policy. |
//! | [`FOREGROUND_SERVICE`] | The `specialUse` foreground service that keeps the process alive while a phone task runs and shows the status notification with its Stop action. |
//! | [`SMOKE_CLASS`] | The debug entry point of [`crate::smoke`]: one native that drives the other two without a model, reached by a broadcast. |
//!
//! The Kotlin originals are `hands/…/XiaomanAccessibilityService.kt` and
//! `app/…/AgentService.kt`. Java keeps the Android objects; Rust keeps the policy (sentinel,
//! operator loop, node compaction) and the `Device` implementation.
//!
//! # Direction of the calls
//!
//! * Rust → Java: `env.call_method(service, name, signature, &[])` with the names and descriptors
//!   of [`SERVICE_METHODS`] and [`FOREGROUND_SERVICE_METHODS`]. The service object is held as a
//!   `GlobalRef` from the moment the JVM calls [`SERVICE_NATIVES`] `nativeOnServiceConnected`.
//! * Java → Rust: `native` methods of [`SERVICE_NATIVES`] and [`FOREGROUND_SERVICE_NATIVES`],
//!   exported as `Java_dev_xiaoman_hands_XiaomanAccessibilityService_nativeOnServiceConnected`,
//!   `Java_dev_xiaoman_app_AgentService_nativeRequestStop`, and so on. They must not be overloaded:
//!   an overload would need the JNI signature suffix.
//!
//! # What Java must send
//!
//! The accessibility service exposes the tree as opaque handles plus one JSON object per node, so
//! no `AccessibilityNodeInfo` pointer ever reaches Rust:
//!
//! * `readRoots()` returns `long[]`: the active window's root first, then the other application and
//!   focused-system windows top-down (dialogs, permission prompts). The IME, the status bar, the
//!   navigation bar and Xiaoman's own overlay are skipped.
//! * `nodeFactsJson(handle)` returns one JSON object, field for field from the node:
//!
//!   ```json
//!   {"text":null,"contentDescription":null,"hintText":"你要去哪儿","resourceId":"com.example:id/n",
//!    "className":"android.widget.EditText","bounds":[40,240,1040,360],"visible":true,
//!    "clickable":false,"longClickable":false,"editable":true,"scrollable":false,
//!    "focused":false,"password":false}
//!   ```
//!
//!   `bounds` is `[left, top, right, bottom]` in screen pixels. Missing keys mean `null`/`false`.
//!   Rust resolves `description` from `contentDescription` and `hintText` with
//!   [`crate::node_compactor::description_or_hint`], so Java must send the raw hint text and not
//!   pre-merge it.
//! * `installedAppsJson()` returns `[{"packageName":"com.autonavi.minimap","label":"高德地图"}]`,
//!   one entry per package (duplicates dropped), sorted by label.
//!
//! # Threading
//!
//! * Rust calls these methods from its own runtime threads. The JNI layer attaches the thread, and
//!   the Java method must hop to the main thread itself where Android requires it
//!   (`takeScreenshot`, `dispatchGesture`, any view or window access). Rust never posts to a
//!   `Looper`.
//! * `captureScreen()` may block for up to about 1.2 s: Android allows one screenshot per second
//!   per service, so the Java side waits for its own interval slot before calling
//!   `takeScreenshot`.
//! * `dispatchStroke()` blocks until `GestureResultCallback` reports completion or cancellation and
//!   returns that result, so Rust sees one boolean per gesture.
//! * Node handles are owned: every non-zero handle must be passed to `nodeRelease` exactly once. The
//!   compactor already reports unkept children through [`crate::node_compactor::NodeTree::release`];
//!   the device releases the roots it did not keep and the whole previous observation.
//! * Failures are values, not exceptions: a refused action returns `false`, a missing value
//!   `null`/`0`, and the JNI layer clears and logs any Java exception it catches.

use crate::app_links::AlarmLink;
use crate::geometry::Stroke;

/// `dev.xiaoman.hands.XiaomanAccessibilityService`: the enabled `AccessibilityService`, declared in
/// the manifest with `android.permission.BIND_ACCESSIBILITY_SERVICE` and configured for gesture
/// dispatch, window content and screenshots (`canPerformGestures`, `canRetrieveWindowContent`,
/// `canTakeScreenshot`, `flagRetrieveInteractiveWindows`, `flagReportViewIds`,
/// `flagIncludeNotImportantViews`).
pub const ACCESSIBILITY_SERVICE: &str = "dev.xiaoman.hands.XiaomanAccessibilityService";

/// `dev.xiaoman.app.AgentService`: the `specialUse` foreground service, started with
/// `ContextCompat.startForegroundService` and stopped with `stopForeground(STOP_FOREGROUND_REMOVE)`
/// once the task is idle.
pub const FOREGROUND_SERVICE: &str = "dev.xiaoman.app.AgentService";

/// An opaque node token: the index of one `AccessibilityNodeInfo` in the service's node table.
/// `0` is "no node"; every other value stays valid until [`AccessibilityHost::release`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeHandle(pub i64);

impl NodeHandle {
    pub const NULL: Self = Self(0);

    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

/// What `captureScreen()` returns: PNG bytes already downscaled to the long edge of
/// [`crate::geometry::DEFAULT_LONG_EDGE`], or no bytes and `secure` when the foreground window is
/// `FLAG_SECURE` (the screenshot then reads as all black rather than failing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenShot {
    pub png: Option<Vec<u8>>,
    pub secure: bool,
}

/// The app in front, from the last `TYPE_WINDOW_STATE_CHANGED` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundApp {
    pub package_name: String,
    pub activity_name: Option<String>,
}

/// One method of a Java class, exactly as `env.call_method` and `GetMethodID` need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JniMethod {
    pub name: &'static str,
    pub signature: &'static str,
}

/// Methods Rust calls on [`ACCESSIBILITY_SERVICE`], in `JNIEnv::call_method` form.
pub const SERVICE_METHODS: &[JniMethod] = &[
    JniMethod {
        name: "isConnected",
        signature: "()Z",
    },
    JniMethod {
        name: "displayWidthPx",
        signature: "()I",
    },
    JniMethod {
        name: "displayHeightPx",
        signature: "()I",
    },
    JniMethod {
        name: "foregroundPackage",
        signature: "()Ljava/lang/String;",
    },
    JniMethod {
        name: "foregroundActivity",
        signature: "()Ljava/lang/String;",
    },
    JniMethod {
        name: "captureScreen",
        signature: "()Ldev/xiaoman/hands/ScreenShot;",
    },
    JniMethod {
        name: "readRoots",
        signature: "()[J",
    },
    JniMethod {
        name: "nodeFactsJson",
        signature: "(J)Ljava/lang/String;",
    },
    JniMethod {
        name: "nodeChildCount",
        signature: "(J)I",
    },
    JniMethod {
        name: "nodeChild",
        signature: "(JI)J",
    },
    JniMethod {
        name: "nodeRelease",
        signature: "(J)V",
    },
    JniMethod {
        name: "nodeRefresh",
        signature: "(J)Z",
    },
    JniMethod {
        name: "nodeClick",
        signature: "(J)Z",
    },
    JniMethod {
        name: "nodeLongClick",
        signature: "(J)Z",
    },
    JniMethod {
        name: "nodeSetText",
        signature: "(JLjava/lang/String;)Z",
    },
    JniMethod {
        name: "nodeImeEnter",
        signature: "(J)Z",
    },
    JniMethod {
        name: "editableFocus",
        signature: "()J",
    },
    JniMethod {
        name: "dispatchStroke",
        signature: "([FJ)Z",
    },
    JniMethod {
        name: "globalBack",
        signature: "()Z",
    },
    JniMethod {
        name: "globalHome",
        signature: "()Z",
    },
    JniMethod {
        name: "launch",
        signature: "(Ljava/lang/String;)Z",
    },
    JniMethod {
        name: "openUri",
        signature: "(Ljava/lang/String;Ljava/lang/String;)Z",
    },
    JniMethod {
        name: "startAlarm",
        signature: "(IILjava/lang/String;)Z",
    },
    JniMethod {
        name: "installedAppsJson",
        signature: "()Ljava/lang/String;",
    },
];

/// `native` methods of [`ACCESSIBILITY_SERVICE`] that the JVM calls; Rust implements and exports
/// them. Signatures are the Java declarations.
pub const SERVICE_NATIVES: &[JniMethod] = &[
    // `onServiceConnected` handed Rust the live service object.
    JniMethod {
        name: "nativeOnServiceConnected",
        signature: "()V",
    },
    // `onUnbind`/`onDestroy`: the `GlobalRef` to the service is dropped, the device reports
    // `DeviceError` until the next connect.
    JniMethod {
        name: "nativeOnServiceUnbound",
        signature: "()V",
    },
    // The overlay's Stop button: `MutableStopSignal::stop`.
    JniMethod {
        name: "nativeRequestStop",
        signature: "()V",
    },
    // A `TYPE_WINDOW_STATE_CHANGED` event, so Rust can report the foreground app even between two
    // observations. Activity may be null when the class is not a resolvable activity.
    JniMethod {
        name: "nativeOnForeground",
        signature: "(Ljava/lang/String;Ljava/lang/String;)V",
    },
];

/// Methods Rust calls on [`FOREGROUND_SERVICE`], all static so Rust never needs the instance.
pub const FOREGROUND_SERVICE_METHODS: &[JniMethod] = &[
    // Starts the service; `false` when the platform refuses a background start (Android 12+). The
    // `Context` argument is the service's application context, held as a `GlobalRef`.
    JniMethod {
        name: "start",
        signature: "(Landroid/content/Context;)Z",
    },
    JniMethod {
        name: "stop",
        signature: "(Landroid/content/Context;)V",
    },
    JniMethod {
        name: "isRunning",
        signature: "()Z",
    },
    // Updates the status line of the notification; no-op when the service is not running.
    JniMethod {
        name: "updateStatus",
        signature: "(Landroid/content/Context;Ljava/lang/String;)V",
    },
];

/// `native` methods of [`FOREGROUND_SERVICE`] that the JVM calls.
pub const FOREGROUND_SERVICE_NATIVES: &[JniMethod] = &[JniMethod {
    name: "nativeRequestStop",
    signature: "()V",
}];

/// `dev.xiaoman.hands.Smoke`: the debug surface of [`crate::smoke`]. `dev.xiaoman.app.SmokeReceiver`
/// calls it on a background thread when `adb shell am broadcast -a dev.xiaoman.smoke` arrives. It
/// is not part of the product's runtime path and owns no policy beyond the script itself.
pub const SMOKE_CLASS: &str = "dev.xiaoman.hands.Smoke";

/// `native` methods of [`SMOKE_CLASS`] that the JVM calls; Rust implements and exports them. The
/// summary is the last line [`crate::smoke`] logs, returned so the receiver can log it too.
pub const SMOKE_NATIVES: &[JniMethod] = &[JniMethod {
    name: "runSmoke",
    signature: "()Ljava/lang/String;",
}];

/// What the Rust side needs from [`ACCESSIBILITY_SERVICE`]. The Android-only `jni_bridge` module
/// implements this trait over `JNIEnv::call_method` with [`SERVICE_METHODS`].
pub trait AccessibilityHost: Send + Sync {
    /// Whether the user has the service enabled and connected.
    fn is_connected(&self) -> bool;

    /// Screen size in pixels of the default display, independent of rotation.
    fn display_size(&self) -> (i32, i32);

    fn foreground(&self) -> Option<ForegroundApp>;

    /// One screenshot, downscaled and PNG-encoded; see [`ScreenShot`].
    fn capture_screen(&self) -> ScreenShot;

    /// Roots of the windows worth reading, active window first.
    fn roots(&self) -> Vec<NodeHandle>;

    /// The raw node facts as JSON; see the module documentation for the keys.
    fn facts_json(&self, node: NodeHandle) -> Option<String>;

    fn child_count(&self, node: NodeHandle) -> usize;

    /// `None` when the child vanished between `child_count` and this call.
    fn child(&self, node: NodeHandle, index: usize) -> Option<NodeHandle>;

    fn release(&self, node: NodeHandle);

    fn refresh(&self, node: NodeHandle) -> bool;

    fn click(&self, node: NodeHandle) -> bool;

    fn long_click(&self, node: NodeHandle) -> bool;

    fn set_text(&self, node: NodeHandle, text: &str) -> bool;

    /// `ACTION_IME_ENTER` on the node, after a `submit` of typed text.
    fn ime_enter(&self, node: NodeHandle) -> bool;

    /// The focused editable node, when the model did not name one.
    fn editable_focus(&self) -> Option<NodeHandle>;

    /// Dispatches one gesture and waits for its result; the point array is
    /// `[x0, y0, x1, y1, …]` in screen pixels.
    fn dispatch_stroke(&self, stroke: &Stroke) -> bool;

    fn global_back(&self) -> bool;

    fn global_home(&self) -> bool;

    /// `getLaunchIntentForPackage` plus `FLAG_ACTIVITY_NEW_TASK`; `false` when the app has no
    /// launcher activity.
    fn launch(&self, package_name: &str) -> bool;

    /// `ACTION_VIEW` for the URI, restricted to `package_name` when given.
    fn open_uri(&self, uri: &str, package_name: Option<&str>) -> bool;

    /// `ACTION_SET_ALARM` with the extras of [`crate::app_links`]; the clock UI is never skipped.
    fn start_alarm(&self, link: &AlarmLink) -> bool;

    /// Launcher activities as JSON; see the module documentation.
    fn installed_apps_json(&self) -> String;
}

/// What the Rust side needs from [`FOREGROUND_SERVICE`].
pub trait ForegroundHost: Send + Sync {
    /// Starts the service; `false` when the platform refused, in which case the task still runs
    /// without a foreground notification.
    fn start(&self) -> bool;

    fn is_running(&self) -> bool;

    fn set_status(&self, status: &str);

    fn stop(&self);
}
