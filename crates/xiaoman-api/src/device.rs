//! The phone's hands and eyes: normalized coordinates, the compact node tree,
//! the action vocabulary and the `Device` trait every platform implements.

use std::error::Error;
use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Normalized coordinates run 0..=1000 on both axes from the top-left corner.
pub const NORM: i32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PointWire", into = "PointWire")]
pub struct Point {
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct PointWire {
    x: i32,
    y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointOutOfRange {
    pub x: i32,
    pub y: i32,
}

impl fmt::Display for PointOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Point out of range 0..{NORM}: ({}, {})", self.x, self.y)
    }
}

impl Error for PointOutOfRange {}

impl Point {
    pub const NORM: i32 = NORM;

    pub fn new(x: i32, y: i32) -> Result<Self, PointOutOfRange> {
        if (0..=NORM).contains(&x) && (0..=NORM).contains(&y) {
            Ok(Self { x, y })
        } else {
            Err(PointOutOfRange { x, y })
        }
    }

    pub fn x(self) -> i32 {
        self.x
    }

    pub fn y(self) -> i32 {
        self.y
    }
}

impl TryFrom<PointWire> for Point {
    type Error = PointOutOfRange;

    fn try_from(wire: PointWire) -> Result<Self, Self::Error> {
        Self::new(wire.x, wire.y)
    }
}

impl From<Point> for PointWire {
    fn from(point: Point) -> Self {
        Self {
            x: point.x,
            y: point.y,
        }
    }
}

/// A rectangle in normalized 0..=1000 coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Bounds {
    pub fn center(&self) -> Point {
        Point::new((self.left + self.right) / 2, (self.top + self.bottom) / 2)
            .expect("bounds center outside 0..1000")
    }
}

/// One element of the compact accessibility tree sent to the operator model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiNode {
    /// Index within [`ScreenState::nodes`]; the model may refer to a node by this id.
    pub id: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(
        default,
        rename = "resourceId",
        skip_serializing_if = "Option::is_none"
    )]
    pub resource_id: Option<String>,
    #[serde(default, rename = "className", skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
    pub bounds: Bounds,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clickable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub editable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub scrollable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub focused: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub password: bool,
}

/// What the agent can see of the phone at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenState {
    /// PNG bytes already downscaled by the [`Device`] (long edge about 1280 px), or
    /// `None` when the screen could not be captured (see [`ScreenState::secure`]).
    pub screenshot_png: Option<Vec<u8>>,
    pub width_px: i32,
    pub height_px: i32,
    pub package_name: Option<String>,
    pub activity_name: Option<String>,
    pub nodes: Vec<UiNode>,
    /// True when the foreground window forbids capture (FLAG_SECURE: payment, password pages).
    pub secure: bool,
    pub captured_at_millis: i64,
}

impl ScreenState {
    pub fn new(
        screenshot_png: Option<Vec<u8>>,
        width_px: i32,
        height_px: i32,
        package_name: Option<String>,
    ) -> Self {
        Self {
            screenshot_png,
            width_px,
            height_px,
            package_name,
            activity_name: None,
            nodes: Vec::new(),
            secure: false,
            captured_at_millis: crate::now_millis(),
        }
    }

    /// Stable fingerprint of what is on screen (package + node texts), used for stuck detection.
    /// Byte-compatible with the Kotlin implementation so traces from both versions compare.
    pub fn fingerprint(&self) -> String {
        let mut s = String::new();
        append_opt(&mut s, self.package_name.as_deref());
        s.push('|');
        append_opt(&mut s, self.activity_name.as_deref());
        for node in &self.nodes {
            s.push('|');
            append_opt(&mut s, node.text.as_deref());
            s.push('/');
            append_opt(&mut s, node.description.as_deref());
            s.push('@');
            let center = node.bounds.center();
            s.push_str(&(center.x() / 20).to_string());
            s.push(',');
            s.push_str(&(center.y() / 20).to_string());
        }
        let hash = java_string_hash(&s);
        render_hash(hash)
    }
}

/// `Integer.toString(hash, 16)`: a minus sign for negatives, and `i32::MIN` prints its magnitude.
fn render_hash(hash: i32) -> String {
    if hash < 0 {
        format!("-{:x}", hash.unsigned_abs())
    } else {
        format!("{hash:x}")
    }
}

fn append_opt(out: &mut String, value: Option<&str>) {
    out.push_str(value.unwrap_or("null"));
}

/// `String.hashCode()` semantics: `s[0]*31^(n-1) + ... + s[n-1]` over UTF-16 code units.
fn java_string_hash(s: &str) -> i32 {
    let mut hash: i32 = 0;
    for unit in s.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(i32::from(unit));
    }
    hash
}

/// One step the operator decides to take. Serialized as `{"type": "...", ...}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Tap {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        point: Option<Point>,
        #[serde(default, rename = "nodeId", skip_serializing_if = "Option::is_none")]
        node_id: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    LongPress {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        point: Option<Point>,
        #[serde(default, rename = "nodeId", skip_serializing_if = "Option::is_none")]
        node_id: Option<i32>,
        #[serde(
            default = "default_long_press_ms",
            rename = "durationMs",
            skip_serializing_if = "is_long_press_ms"
        )]
        duration_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Swipe {
        from: Point,
        to: Point,
        #[serde(
            default = "default_swipe_ms",
            rename = "durationMs",
            skip_serializing_if = "is_swipe_ms"
        )]
        duration_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Types into the focused editable field, or into `nodeId` when given.
    #[serde(rename = "type")]
    TypeText {
        text: String,
        #[serde(default, rename = "nodeId", skip_serializing_if = "Option::is_none")]
        node_id: Option<i32>,
        #[serde(default, skip_serializing_if = "is_false")]
        submit: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Back {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Home {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    #[serde(rename = "launch")]
    LaunchApp {
        #[serde(rename = "packageName")]
        package_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    #[serde(rename = "deeplink")]
    OpenDeeplink {
        uri: String,
        #[serde(
            default,
            rename = "packageName",
            skip_serializing_if = "Option::is_none"
        )]
        package_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Wait {
        #[serde(default = "default_wait_ms", skip_serializing_if = "is_wait_ms")]
        millis: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Hands control to the user (login, captcha, secure page) or asks a question.
    AskUser {
        question: String,
        #[serde(default, skip_serializing_if = "is_false")]
        takeover: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Finish {
        success: bool,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

fn default_long_press_ms() -> i64 {
    800
}

fn default_swipe_ms() -> i64 {
    300
}

fn default_wait_ms() -> i64 {
    1000
}

fn is_long_press_ms(value: &i64) -> bool {
    *value == default_long_press_ms()
}

fn is_swipe_ms(value: &i64) -> bool {
    *value == default_swipe_ms()
}

fn is_wait_ms(value: &i64) -> bool {
    *value == default_wait_ms()
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Action {
    /// Free-form words describing what the action aims at, e.g. "确认呼叫 按钮".
    pub fn target(&self) -> Option<&str> {
        let target = match self {
            Self::Tap { target, .. }
            | Self::LongPress { target, .. }
            | Self::Swipe { target, .. }
            | Self::TypeText { target, .. }
            | Self::Back { target, .. }
            | Self::Home { target, .. }
            | Self::LaunchApp { target, .. }
            | Self::OpenDeeplink { target, .. }
            | Self::Wait { target, .. }
            | Self::AskUser { target, .. }
            | Self::Finish { target, .. } => target,
        };
        target.as_deref()
    }

    /// Why the model chose this step; shown to the user and kept in the trace.
    pub fn reason(&self) -> Option<&str> {
        let reason = match self {
            Self::Tap { reason, .. }
            | Self::LongPress { reason, .. }
            | Self::Swipe { reason, .. }
            | Self::TypeText { reason, .. }
            | Self::Back { reason, .. }
            | Self::Home { reason, .. }
            | Self::LaunchApp { reason, .. }
            | Self::OpenDeeplink { reason, .. }
            | Self::Wait { reason, .. }
            | Self::AskUser { reason, .. }
            | Self::Finish { reason, .. } => reason,
        };
        reason.as_deref()
    }

    /// Terminal actions end the operator loop instead of reaching the device.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::AskUser { .. } | Self::Finish { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ActionResult {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: Some(message.into()),
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub package_name: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceError(pub String);

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for DeviceError {}

/// The phone's hands and eyes. Implemented on Android by the accessibility
/// service, on iOS by the WebDriverAgent client, and faked in tests.
#[async_trait]
pub trait Device: Send + Sync {
    async fn observe(&self) -> Result<ScreenState, DeviceError>;

    /// Executes one non-terminal action (not [`Action::AskUser`] or [`Action::Finish`]).
    async fn perform(&self, action: Action) -> ActionResult;

    async fn installed_apps(&self) -> Vec<AppInfo>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_out_of_range_point() {
        assert!(Point::new(1001, 0).is_err());
        assert!(Point::new(0, -1).is_err());
        assert!(Point::new(0, 0).is_ok());
        assert!(Point::new(1000, 1000).is_ok());
    }

    #[test]
    fn fingerprint_matches_java_string_hash() {
        let mut screen = ScreenState::new(None, 1080, 2400, Some("com.example".into()));
        screen.activity_name = Some("Main".into());
        screen.nodes.push(UiNode {
            id: 0,
            text: Some("确认".into()),
            description: None,
            resource_id: None,
            class_name: None,
            bounds: Bounds {
                left: 0,
                top: 0,
                right: 200,
                bottom: 100,
            },
            clickable: true,
            editable: false,
            scrollable: false,
            focused: false,
            password: false,
        });
        // Java: "com.example|Main|确认/null@5,2".hashCode() == -584996425,
        // Integer.toString(-584996425, 16) == "-22de5649".
        assert_eq!(screen.fingerprint(), "-22de5649");
    }

    #[test]
    fn the_hash_is_rendered_like_javas_integer_to_string() {
        assert_eq!(render_hash(0), "0");
        assert_eq!(render_hash(1), "1");
        assert_eq!(render_hash(-1), "-1");
        assert_eq!(render_hash(-584996425), "-22de5649");
        assert_eq!(render_hash(i32::MIN), "-80000000");
    }
}
