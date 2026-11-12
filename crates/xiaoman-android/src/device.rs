//! The Android [`Device`]: the middle layer Kotlin's `AccessibilityDevice` used to provide.
//!
//! [`AndroidDevice`] owns the policy that does not belong in Java: the compact node list, the
//! screenshot interval, the `nodeId`-or-point fallback of taps, the text-entry sequence and the
//! ownership of the node handles handed out by [`AccessibilityHost`]. Everything Android-specific
//! stays behind the host trait ([`crate::bridge`]); the device itself runs against a fake on the
//! host and against the JNI bridge on the phone.
//!
//! # Node handles
//!
//! A handle is owned from the moment a host call returns it until it is released. The device
//! releases:
//!
//! * every child the compactor did not keep — through [`NodeTree::release`];
//! * every root the compactor did not keep — after compaction, comparing handles;
//! * the whole previous observation, once the new one replaced it;
//! * a transient `editableFocus()` handle, after the text was typed or typed and submitted.
//!
//! Each handle is released exactly once: a release list deduplicates it inside one observation and
//! between the observation it replaces.
//!
//! # Screenshots
//!
//! Android allows one `takeScreenshot` per second per service. The device remembers when the last
//! capture returned (through [`Clock`], monotonic milliseconds like `SystemClock.uptimeMillis`)
//! and waits out the rest of [`SCREENSHOT_INTERVAL_MS`] before the next capture. The clock is
//! injectable, so tests never sleep for real.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;
use xiaoman_api::{Action, ActionResult, AppInfo, Device, DeviceError, Point, ScreenState, UiNode};

use crate::app_links::link_for;
use crate::bridge::{AccessibilityHost, NodeHandle, ScreenShot};
use crate::geometry::{self, PxRect, Stroke};
use crate::node_compactor::{
    DEFAULT_MAX_NODES, NodeCompactor, NodeFacts, NodeTree, description_or_hint,
};

/// `AccessibilityDevice.NOT_CONNECTED`, byte for byte: the user has not enabled the service.
pub const NOT_CONNECTED: &str = "无障碍服务未开启";

/// What the Kotlin device answers for actions the operator loop handles itself.
pub const NOT_A_DEVICE_ACTION: &str = "not a device action";

/// Minimum spacing of two screenshots, in milliseconds: Android's own limit is one per second.
pub const SCREENSHOT_INTERVAL_MS: i64 = 1050;

/// Pause after tapping a field that was not focused yet, before typing into it.
pub const FOCUS_DELAY_MS: i64 = 400;

/// Pause between setting the text and the IME enter that submits it.
pub const SUBMIT_DELAY_MS: i64 = 150;

/// Longest [`Action::Wait`] honoured, in milliseconds.
pub const MAX_WAIT_MS: i64 = 30_000;

/// Pause after every action except [`Action::Wait`], so the screen can settle before the next
/// observation; the Kotlin default.
pub const DEFAULT_SETTLE_MS: i64 = 600;

/// Monotonic time and the sleep between screenshots. Replace it in tests to avoid real waiting.
#[async_trait]
pub trait Clock: Send + Sync {
    /// Monotonic milliseconds, like Android's `SystemClock.uptimeMillis`.
    fn now_millis(&self) -> i64;

    /// Waits `millis`; non-positive durations return immediately.
    async fn sleep(&self, millis: i64);
}

/// The real clock: monotonic milliseconds since construction and the async runtime's timer.
#[derive(Debug, Clone)]
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Clock for SystemClock {
    fn now_millis(&self) -> i64 {
        self.start.elapsed().as_millis().min(i64::MAX as u128) as i64
    }

    async fn sleep(&self, millis: i64) {
        if millis > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(millis as u64)).await;
        }
    }
}

/// Settings of [`AndroidDevice`], mirroring the Kotlin constructor parameters.
#[derive(Clone)]
pub struct AndroidDeviceOptions {
    /// Cap of the compact node list, Kotlin's `maxNodes`.
    pub max_nodes: usize,
    /// Pause after every action except [`Action::Wait`]; `0` disables it.
    pub settle_millis: i64,
    pub clock: Arc<dyn Clock>,
}

impl Default for AndroidDeviceOptions {
    fn default() -> Self {
        Self {
            max_nodes: DEFAULT_MAX_NODES,
            settle_millis: DEFAULT_SETTLE_MS,
            clock: Arc::new(SystemClock::new()),
        }
    }
}

/// One kept node of the latest observation: the handle to act on and the facts the compactor
/// dropped from [`UiNode`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct KeptNode {
    handle: NodeHandle,
    bounds_px: PxRect,
    clickable: bool,
    long_clickable: bool,
    editable: bool,
}

#[derive(Default)]
struct State {
    latest: Vec<KeptNode>,
    /// Monotonic time of the last capture; `None` before the first one.
    last_screenshot_at: Option<i64>,
}

/// The phone's hands and eyes over an [`AccessibilityHost`].
pub struct AndroidDevice {
    host: Arc<dyn AccessibilityHost>,
    compactor: NodeCompactor,
    clock: Arc<dyn Clock>,
    settle_millis: i64,
    state: tokio::sync::Mutex<State>,
}

impl AndroidDevice {
    /// A device with the default limits, a 600 ms settle pause and the real clock.
    pub fn new(host: Arc<dyn AccessibilityHost>) -> Self {
        Self::with_options(host, AndroidDeviceOptions::default())
    }

    pub fn with_options(host: Arc<dyn AccessibilityHost>, options: AndroidDeviceOptions) -> Self {
        Self {
            host,
            compactor: NodeCompactor::with_max_nodes(options.max_nodes),
            clock: options.clock,
            settle_millis: options.settle_millis,
            state: tokio::sync::Mutex::new(State::default()),
        }
    }

    pub fn host(&self) -> &Arc<dyn AccessibilityHost> {
        &self.host
    }

    /// Captures one screenshot, waiting out the interval Android enforces. Must be called with the
    /// state lock held so two observations cannot capture inside the same slot.
    async fn capture(&self, state: &mut State) -> (Option<Vec<u8>>, bool) {
        if let Some(last) = state.last_screenshot_at {
            let wait = last + SCREENSHOT_INTERVAL_MS - self.clock.now_millis();
            if wait > 0 {
                self.clock.sleep(wait).await;
            }
        }
        let shot = self.host.capture_screen();
        state.last_screenshot_at = Some(self.clock.now_millis());
        encode_screenshot(shot)
    }

    /// Reads the compact node list, replaces the latest observation and returns the model's nodes.
    fn read_nodes(&self, state: &mut State, width_px: i32, height_px: i32) -> Vec<UiNode> {
        let roots = self.host.roots();
        let tree = HostTree::new(self.host.as_ref());
        let kept = self.compactor.compact(&roots, &tree, width_px, height_px);

        // One release bookkeeping for the whole observation: a handle never reaches the host twice,
        // even when a root and a kept node of the previous observation coincide.
        let mut released: HashSet<i64> = HashSet::new();
        for root in &roots {
            let is_kept = kept.iter().any(|node| node.source == *root);
            if !is_kept && released.insert(root.0) {
                self.host.release(*root);
            }
        }

        let entries: Vec<KeptNode> = kept
            .iter()
            .map(|node| {
                let facts = tree.facts_of(node.source).unwrap_or_default();
                KeptNode {
                    handle: node.source,
                    bounds_px: node.bounds_px,
                    clickable: facts.clickable,
                    long_clickable: facts.long_clickable,
                    editable: facts.editable,
                }
            })
            .collect();

        for previous in std::mem::replace(&mut state.latest, entries) {
            if released.insert(previous.handle.0) {
                self.host.release(previous.handle);
            }
        }

        kept.into_iter().map(|node| node.ui).collect()
    }

    fn facts_of(&self, node: NodeHandle) -> Option<NodeFacts> {
        self.host
            .facts_json(node)
            .and_then(|json| facts_from_json(&json))
    }

    async fn execute(&self, state: &State, action: &Action) -> ActionResult {
        match action {
            Action::Tap { point, node_id, .. } => {
                self.press(state, *node_id, *point, false, geometry::TAP_MS)
                    .await
            }
            Action::LongPress {
                point,
                node_id,
                duration_ms,
                ..
            } => {
                self.press(
                    state,
                    *node_id,
                    *point,
                    true,
                    geometry::long_press_ms(*duration_ms),
                )
                .await
            }
            Action::Swipe {
                from,
                to,
                duration_ms,
                ..
            } => {
                let (width_px, height_px) = self.host.display_size();
                let stroke = Stroke::swipe(*from, *to, width_px, height_px, *duration_ms);
                self.dispatch(&stroke, "swipe")
            }
            Action::TypeText {
                text,
                node_id,
                submit,
                ..
            } => self.type_text(state, *node_id, text, *submit).await,
            Action::Back { .. } => global(self.host.global_back(), "back"),
            Action::Home { .. } => global(self.host.global_home(), "home"),
            Action::LaunchApp { package_name, .. } => self.launch(package_name),
            Action::OpenDeeplink {
                uri, package_name, ..
            } => self.deeplink(uri, package_name.as_deref()),
            Action::Wait { millis, .. } => {
                self.clock.sleep((*millis).clamp(0, MAX_WAIT_MS)).await;
                ActionResult {
                    ok: true,
                    message: None,
                }
            }
            Action::AskUser { .. } | Action::Finish { .. } => {
                ActionResult::failed(NOT_A_DEVICE_ACTION)
            }
        }
    }

    /// Taps a `nodeId` through its handle and falls back to the node's center, or taps a point.
    async fn press(
        &self,
        state: &State,
        node_id: Option<i32>,
        point: Option<Point>,
        long: bool,
        duration_ms: i64,
    ) -> ActionResult {
        let entry = node_id.and_then(|id| lookup(state, id));
        if let Some(id) = node_id {
            if entry.is_none() && point.is_none() {
                return ActionResult::failed(format!("unknown nodeId {id}"));
            }
        }
        if let Some(entry) = entry {
            let id = node_id.expect("an entry only exists for a nodeId");
            let fresh = self.host.refresh(entry.handle);
            let live = fresh.then(|| self.facts_of(entry.handle)).flatten();
            let can_act = live
                .as_ref()
                .map(|facts| {
                    if long {
                        facts.long_clickable
                    } else {
                        facts.clickable
                    }
                })
                .unwrap_or(if long {
                    entry.long_clickable
                } else {
                    entry.clickable
                });
            if fresh && can_act {
                let acted = if long {
                    self.host.long_click(entry.handle)
                } else {
                    self.host.click(entry.handle)
                };
                if acted {
                    let verb = if long { "long_click" } else { "click" };
                    return ActionResult::ok(format!("{verb} node {id}"));
                }
            }
            // Not clickable itself (text inside a clickable parent), stale, or refused: press its
            // center. A refreshed node reports current bounds; a stale one only has the observed ones.
            let rect = live
                .as_ref()
                .map(|facts| facts.bounds_px)
                .filter(|rect| *rect != PxRect::default())
                .unwrap_or(entry.bounds_px);
            let (x, y) = rect.center();
            return self.press_at(x, y, duration_ms, &format!("node {id}"));
        }
        let Some(point) = point else {
            return ActionResult::failed("tap needs nodeId or point");
        };
        let (width_px, height_px) = self.host.display_size();
        let (x, y) = geometry::denormalize(point, width_px, height_px);
        self.press_at(x, y, duration_ms, &format!("({},{})", point.x(), point.y()))
    }

    fn press_at(&self, x: f32, y: f32, duration_ms: i64, what: &str) -> ActionResult {
        let stroke = Stroke {
            points: vec![(x, y)],
            duration_ms,
        };
        self.dispatch(&stroke, &format!("press {what}"))
    }

    fn dispatch(&self, stroke: &Stroke, what: &str) -> ActionResult {
        if self.host.dispatch_stroke(stroke) {
            ActionResult::ok(what)
        } else {
            ActionResult::failed(format!("{what}: gesture cancelled"))
        }
    }

    /// Focuses a field when it is not editable yet, sets the text and submits it with the IME when
    /// asked to.
    async fn type_text(
        &self,
        state: &State,
        node_id: Option<i32>,
        text: &str,
        submit: bool,
    ) -> ActionResult {
        let mut target = None;
        if let Some(id) = node_id {
            let Some(entry) = lookup(state, id) else {
                return ActionResult::failed(format!("unknown nodeId {id}"));
            };
            let fresh = self.host.refresh(entry.handle);
            let editable = fresh
                && self
                    .facts_of(entry.handle)
                    .map(|facts| facts.editable)
                    .unwrap_or(entry.editable);
            if editable {
                target = Some(entry.handle);
            } else {
                // Focus the field first: search bars are often a clickable placeholder that opens
                // the real input. The Kotlin device ignores the press result.
                let _ = self
                    .press(state, Some(id), None, false, geometry::TAP_MS)
                    .await;
                self.clock.sleep(FOCUS_DELAY_MS).await;
            }
        }
        // `editable_focus` hands out a transient handle the device owns, unlike the kept nodes.
        let focused = match target {
            Some(_) => None,
            None => match self.host.editable_focus() {
                Some(handle) => Some(handle),
                None => return ActionResult::failed("no editable field focused"),
            },
        };
        let node = target.or(focused).expect("target or focused handle");
        let result = self.type_into(node, text, submit).await;
        if let Some(handle) = focused {
            self.host.release(handle);
        }
        result
    }

    async fn type_into(&self, node: NodeHandle, text: &str, submit: bool) -> ActionResult {
        if !self.host.set_text(node, text) {
            return ActionResult::failed("set text refused");
        }
        if !submit {
            return ActionResult::ok("typed");
        }
        self.clock.sleep(SUBMIT_DELAY_MS).await;
        if self.host.ime_enter(node) {
            ActionResult::ok("typed and submitted")
        } else {
            ActionResult::failed("text set, but IME enter refused")
        }
    }

    fn launch(&self, package_name: &str) -> ActionResult {
        if self.host.launch(package_name) {
            ActionResult::ok(format!("launched {package_name}"))
        } else {
            ActionResult::failed(format!("未找到应用 {package_name}"))
        }
    }

    fn deeplink(&self, uri: &str, package_name: Option<&str>) -> ActionResult {
        let scheme = uri
            .split_once(':')
            .map(|(scheme, _)| scheme)
            .unwrap_or_default();
        if scheme.is_empty() {
            return ActionResult::failed(format!("invalid uri {uri}"));
        }
        let opened = match link_for(uri) {
            Ok(Some(link)) => link
                .alarm()
                .map(|alarm| self.host.start_alarm(alarm))
                .unwrap_or(false),
            Ok(None) => self.host.open_uri(uri, package_name),
            Err(error) => return ActionResult::failed(error.to_string()),
        };
        if opened {
            ActionResult::ok(format!("opened {uri}"))
        } else {
            ActionResult::failed(format!("no app can open it: {uri}"))
        }
    }
}

#[async_trait]
impl Device for AndroidDevice {
    async fn observe(&self) -> Result<ScreenState, DeviceError> {
        if !self.host.is_connected() {
            return Err(DeviceError(NOT_CONNECTED.to_string()));
        }
        let mut state = self.state.lock().await;
        let (width_px, height_px) = self.host.display_size();
        let (screenshot_png, secure) = self.capture(&mut state).await;
        let foreground = self.host.foreground();
        let nodes = self.read_nodes(&mut state, width_px, height_px);
        let (package_name, activity_name) = match foreground {
            Some(app) => (Some(app.package_name), app.activity_name),
            None => (None, None),
        };
        Ok(ScreenState {
            screenshot_png,
            width_px,
            height_px,
            package_name,
            activity_name,
            nodes,
            secure,
            captured_at_millis: xiaoman_api::now_millis(),
        })
    }

    async fn perform(&self, action: Action) -> ActionResult {
        if action.is_terminal() {
            return ActionResult::failed(NOT_A_DEVICE_ACTION);
        }
        if !self.host.is_connected() {
            return ActionResult::failed(NOT_CONNECTED);
        }
        let result = {
            let state = self.state.lock().await;
            self.execute(&state, &action).await
        };
        // Kotlin waits outside the lock and never after Wait.
        if !matches!(action, Action::Wait { .. }) && self.settle_millis > 0 {
            self.clock.sleep(self.settle_millis).await;
        }
        result
    }

    async fn installed_apps(&self) -> Vec<AppInfo> {
        parse_installed_apps(&self.host.installed_apps_json())
    }
}

fn lookup(state: &State, node_id: i32) -> Option<&KeptNode> {
    usize::try_from(node_id)
        .ok()
        .and_then(|index| state.latest.get(index))
}

fn global(performed: bool, what: &str) -> ActionResult {
    if performed {
        ActionResult::ok(what)
    } else {
        ActionResult::failed(format!("{what} refused"))
    }
}

/// [`NodeTree`] over the host's handles, caching the facts the compactor asks for so the kept nodes
/// can be rebuilt without a second host call.
struct HostTree<'a> {
    host: &'a dyn AccessibilityHost,
    facts: std::cell::RefCell<HashMap<NodeHandle, NodeFacts>>,
}

impl<'a> HostTree<'a> {
    fn new(host: &'a dyn AccessibilityHost) -> Self {
        Self {
            host,
            facts: std::cell::RefCell::new(HashMap::new()),
        }
    }

    fn facts_of(&self, node: NodeHandle) -> Option<NodeFacts> {
        self.facts.borrow().get(&node).cloned()
    }
}

impl NodeTree<NodeHandle> for HostTree<'_> {
    fn facts(&self, node: &NodeHandle) -> NodeFacts {
        let facts = self
            .host
            .facts_json(*node)
            .and_then(|json| facts_from_json(&json))
            .unwrap_or_default();
        self.facts.borrow_mut().insert(*node, facts.clone());
        facts
    }

    fn child_count(&self, node: &NodeHandle) -> usize {
        self.host.child_count(*node)
    }

    fn child(&self, node: &NodeHandle, index: usize) -> Option<NodeHandle> {
        self.host.child(*node, index)
    }

    fn release(&self, node: NodeHandle) {
        self.host.release(node);
    }
}

/// One node facts object of `nodeFactsJson`, as [`crate::bridge`] documents it. Missing keys mean
/// `null`/`false`; the description is resolved from `contentDescription` and `hintText` here, not
/// by Java.
pub fn facts_from_json(json: &str) -> Option<NodeFacts> {
    let value: Value = serde_json::from_str(json).ok()?;
    if !value.is_object() {
        return None;
    }
    let text = string_field(&value, "text");
    let editable = bool_field(&value, "editable");
    let description = description_or_hint(
        string_field(&value, "contentDescription").as_deref(),
        string_field(&value, "hintText").as_deref(),
        text.as_deref(),
        editable,
    );
    Some(NodeFacts {
        text,
        description,
        resource_id: string_field(&value, "resourceId"),
        class_name: string_field(&value, "className"),
        bounds_px: bounds_field(&value, "bounds"),
        visible: bool_field(&value, "visible"),
        clickable: bool_field(&value, "clickable"),
        long_clickable: bool_field(&value, "longClickable"),
        editable,
        scrollable: bool_field(&value, "scrollable"),
        focused: bool_field(&value, "focused"),
        password: bool_field(&value, "password"),
    })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn bool_field(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn bounds_field(value: &Value, key: &str) -> PxRect {
    let Some(items) = value.get(key).and_then(Value::as_array) else {
        return PxRect::default();
    };
    let axis = |index: usize| {
        items
            .get(index)
            .and_then(|item| {
                item.as_i64()
                    .or_else(|| item.as_f64().map(|number| number.round() as i64))
            })
            .and_then(|number| i32::try_from(number).ok())
            .unwrap_or_default()
    };
    PxRect::new(axis(0), axis(1), axis(2), axis(3))
}

/// The JSON of `installedAppsJson`: launcher activities, one per package, sorted by label.
fn parse_installed_apps(json: &str) -> Vec<AppInfo> {
    let Ok(Value::Array(items)) = serde_json::from_str(json) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut apps: Vec<AppInfo> = items
        .iter()
        .filter_map(|item| {
            let package_name = item.get("packageName").and_then(Value::as_str)?;
            if package_name.trim().is_empty() || !seen.insert(package_name.to_string()) {
                return None;
            }
            let label = item
                .get("label")
                .and_then(Value::as_str)
                .filter(|label| !label.trim().is_empty())
                .unwrap_or(package_name);
            Some(AppInfo {
                package_name: package_name.to_string(),
                label: label.to_string(),
            })
        })
        .collect();
    apps.sort_by(|left, right| left.label.cmp(&right.label));
    apps
}

/// PNG bytes of a screenshot for the operator model, plus the secure flag. A secure window yields
/// no bytes; an all-black frame (Android 11–13 capture `FLAG_SECURE` windows as black) is reported
/// as secure too, exactly as the Kotlin `encode` does.
fn encode_screenshot(shot: ScreenShot) -> (Option<Vec<u8>>, bool) {
    if shot.secure {
        return (None, true);
    }
    let Some(png) = shot.png else {
        return (None, false);
    };
    let Some(image) = decode_rgb(&png) else {
        return (None, false);
    };
    let (width, height) = image.dimensions();
    let Ok((target_width, target_height)) = geometry::target_size(width as i32, height as i32)
    else {
        return (None, false);
    };
    let image = if (target_width as u32, target_height as u32) == (width, height) {
        image
    } else {
        image::imageops::resize(
            &image,
            target_width as u32,
            target_height as u32,
            image::imageops::FilterType::Triangle,
        )
    };
    let pixels: Vec<u32> = image
        .pixels()
        .map(|pixel| u32::from(pixel[0]) << 16 | u32::from(pixel[1]) << 8 | u32::from(pixel[2]))
        .collect();
    if geometry::looks_all_black(&pixels, image.width() as usize, image.height() as usize) {
        return (None, true);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    if image::DynamicImage::ImageRgb8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .is_err()
    {
        return (None, false);
    }
    (Some(out.into_inner()), false)
}

fn decode_rgb(png: &[u8]) -> Option<image::RgbImage> {
    let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    Some(decoded.to_rgb8())
}
