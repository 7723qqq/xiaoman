//! [`AndroidDevice`] against a fake accessibility host: the Kotlin `AccessibilityDevice` behavior
//! — compaction, the screenshot slot, secure frames, nodeId-or-point taps, text entry, handle
//! ownership and installed apps — without an Android device.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use xiaoman_android::bridge::{AccessibilityHost, ForegroundApp, NodeHandle, ScreenShot};
use xiaoman_android::device::{
    AndroidDevice, AndroidDeviceOptions, Clock, NOT_CONNECTED, SCREENSHOT_INTERVAL_MS,
};
use xiaoman_android::geometry::Stroke;
use xiaoman_api::{Action, ActionResult, AppInfo, Bounds, Device, Point};

// ---------------------------------------------------------------------------------------------
// The fake host
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct FakeNode {
    text: Option<String>,
    description: Option<String>,
    hint: Option<String>,
    resource_id: Option<String>,
    class_name: Option<String>,
    bounds: (i32, i32, i32, i32),
    visible: bool,
    clickable: bool,
    long_clickable: bool,
    editable: bool,
    scrollable: bool,
    focused: bool,
    password: bool,
    children: Vec<i64>,
}

impl Default for FakeNode {
    fn default() -> Self {
        Self {
            text: None,
            description: None,
            hint: None,
            resource_id: None,
            class_name: None,
            bounds: (0, 0, 0, 0),
            visible: true,
            clickable: false,
            long_clickable: false,
            editable: false,
            scrollable: false,
            focused: false,
            password: false,
            children: Vec::new(),
        }
    }
}

impl FakeNode {
    /// The `nodeFactsJson` object of the JNI contract.
    fn json(&self) -> String {
        serde_json::json!({
            "text": self.text,
            "contentDescription": self.description,
            "hintText": self.hint,
            "resourceId": self.resource_id,
            "className": self.class_name,
            "bounds": [self.bounds.0, self.bounds.1, self.bounds.2, self.bounds.3],
            "visible": self.visible,
            "clickable": self.clickable,
            "longClickable": self.long_clickable,
            "editable": self.editable,
            "scrollable": self.scrollable,
            "focused": self.focused,
            "password": self.password,
        })
        .to_string()
    }
}

struct FakeInner {
    connected: bool,
    display: (i32, i32),
    foreground: Option<ForegroundApp>,
    screenshot: ScreenShot,
    nodes: HashMap<i64, FakeNode>,
    roots: Vec<i64>,
    focus: Option<i64>,
    gesture_ok: bool,
    installed_json: String,
    releases: Vec<i64>,
    clicks: Vec<i64>,
    long_clicks: Vec<i64>,
    refreshes: Vec<i64>,
    set_texts: Vec<(i64, String)>,
    ime_enters: Vec<i64>,
    strokes: Vec<Stroke>,
    captures: usize,
    launches: Vec<String>,
}

impl Default for FakeInner {
    fn default() -> Self {
        Self {
            connected: false,
            display: (0, 0),
            foreground: None,
            screenshot: ScreenShot {
                png: None,
                secure: false,
            },
            nodes: HashMap::new(),
            roots: Vec::new(),
            focus: None,
            gesture_ok: true,
            installed_json: String::new(),
            releases: Vec::new(),
            clicks: Vec::new(),
            long_clicks: Vec::new(),
            refreshes: Vec::new(),
            set_texts: Vec::new(),
            ime_enters: Vec::new(),
            strokes: Vec::new(),
            captures: 0,
            launches: Vec::new(),
        }
    }
}

struct FakeHost {
    inner: Mutex<FakeInner>,
}

impl FakeHost {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(FakeInner::default()),
        })
    }

    fn inner(&self) -> MutexGuard<'_, FakeInner> {
        self.inner.lock().unwrap()
    }
}

impl AccessibilityHost for FakeHost {
    fn is_connected(&self) -> bool {
        self.inner().connected
    }

    fn display_size(&self) -> (i32, i32) {
        self.inner().display
    }

    fn foreground(&self) -> Option<ForegroundApp> {
        self.inner().foreground.clone()
    }

    fn capture_screen(&self) -> ScreenShot {
        let mut inner = self.inner();
        inner.captures += 1;
        inner.screenshot.clone()
    }

    fn roots(&self) -> Vec<NodeHandle> {
        self.inner().roots.iter().copied().map(NodeHandle).collect()
    }

    fn facts_json(&self, node: NodeHandle) -> Option<String> {
        self.inner().nodes.get(&node.0).map(FakeNode::json)
    }

    fn child_count(&self, node: NodeHandle) -> usize {
        self.inner()
            .nodes
            .get(&node.0)
            .map(|node| node.children.len())
            .unwrap_or_default()
    }

    fn child(&self, node: NodeHandle, index: usize) -> Option<NodeHandle> {
        let inner = self.inner();
        inner
            .nodes
            .get(&node.0)?
            .children
            .get(index)
            .copied()
            .map(NodeHandle)
    }

    fn release(&self, node: NodeHandle) {
        self.inner().releases.push(node.0);
    }

    fn refresh(&self, node: NodeHandle) -> bool {
        let mut inner = self.inner();
        inner.refreshes.push(node.0);
        inner.nodes.contains_key(&node.0)
    }

    fn click(&self, node: NodeHandle) -> bool {
        let mut inner = self.inner();
        inner.clicks.push(node.0);
        inner
            .nodes
            .get(&node.0)
            .map(|node| node.clickable)
            .unwrap_or(false)
    }

    fn long_click(&self, node: NodeHandle) -> bool {
        let mut inner = self.inner();
        inner.long_clicks.push(node.0);
        inner
            .nodes
            .get(&node.0)
            .map(|node| node.long_clickable)
            .unwrap_or(false)
    }

    fn set_text(&self, node: NodeHandle, text: &str) -> bool {
        self.inner().set_texts.push((node.0, text.to_string()));
        true
    }

    fn ime_enter(&self, node: NodeHandle) -> bool {
        self.inner().ime_enters.push(node.0);
        true
    }

    fn editable_focus(&self) -> Option<NodeHandle> {
        self.inner().focus.map(NodeHandle)
    }

    fn dispatch_stroke(&self, stroke: &Stroke) -> bool {
        let mut inner = self.inner();
        inner.strokes.push(stroke.clone());
        inner.gesture_ok
    }

    fn global_back(&self) -> bool {
        true
    }

    fn global_home(&self) -> bool {
        true
    }

    fn launch(&self, package_name: &str) -> bool {
        self.inner().launches.push(package_name.to_string());
        true
    }

    fn open_uri(&self, _uri: &str, _package_name: Option<&str>) -> bool {
        true
    }

    fn start_alarm(&self, _link: &xiaoman_android::AlarmLink) -> bool {
        true
    }

    fn installed_apps_json(&self) -> String {
        self.inner().installed_json.clone()
    }
}

// ---------------------------------------------------------------------------------------------
// The fake clock
// ---------------------------------------------------------------------------------------------

struct FakeClock {
    now: AtomicI64,
    sleeps: Mutex<Vec<i64>>,
}

impl FakeClock {
    fn new(now: i64) -> Arc<Self> {
        Arc::new(Self {
            now: AtomicI64::new(now),
            sleeps: Mutex::new(Vec::new()),
        })
    }

    fn set_now(&self, now: i64) {
        self.now.store(now, Ordering::SeqCst);
    }

    fn sleeps(&self) -> Vec<i64> {
        self.sleeps.lock().unwrap().clone()
    }
}

#[async_trait]
impl Clock for FakeClock {
    fn now_millis(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }

    async fn sleep(&self, millis: i64) {
        self.sleeps.lock().unwrap().push(millis);
        self.now.fetch_add(millis, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn device(host: &Arc<FakeHost>, clock: &Arc<FakeClock>, max_nodes: usize) -> AndroidDevice {
    let host: Arc<dyn AccessibilityHost> = host.clone();
    let clock: Arc<dyn Clock> = clock.clone();
    AndroidDevice::with_options(
        host,
        AndroidDeviceOptions {
            max_nodes,
            settle_millis: 0,
            clock,
        },
    )
}

fn connected(host: &Arc<FakeHost>) {
    let mut inner = host.inner();
    inner.connected = true;
    inner.display = (1080, 2400);
}

fn png(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb(color));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("PNG encode");
    out.into_inner()
}

fn tap(node_id: i32) -> Action {
    Action::Tap {
        point: None,
        node_id: Some(node_id),
        target: None,
        reason: None,
    }
}

fn type_text(node_id: i32, text: &str, submit: bool) -> Action {
    Action::TypeText {
        text: text.to_string(),
        node_id: Some(node_id),
        submit,
        target: None,
        reason: None,
    }
}

/// Root 1 with a search field (2), a clickable "快车" button (3) and a plain label (4).
fn search_screen(host: &Arc<FakeHost>) {
    let mut inner = host.inner();
    inner.roots = vec![1];
    inner.nodes.insert(
        1,
        FakeNode {
            class_name: Some("android.widget.FrameLayout".to_string()),
            bounds: (0, 0, 1080, 2400),
            children: vec![2, 3, 4],
            ..FakeNode::default()
        },
    );
    inner.nodes.insert(
        2,
        FakeNode {
            hint: Some("你要去哪儿".to_string()),
            resource_id: Some("com.example:id/n".to_string()),
            class_name: Some("android.widget.EditText".to_string()),
            bounds: (40, 240, 1040, 360),
            editable: true,
            ..FakeNode::default()
        },
    );
    inner.nodes.insert(
        3,
        FakeNode {
            text: Some("快车".to_string()),
            class_name: Some("android.widget.TextView".to_string()),
            bounds: (0, 2160, 540, 2400),
            clickable: true,
            ..FakeNode::default()
        },
    );
    inner.nodes.insert(
        4,
        FakeNode {
            text: Some("其他".to_string()),
            class_name: Some("android.widget.TextView".to_string()),
            bounds: (0, 0, 100, 100),
            ..FakeNode::default()
        },
    );
}

// ---------------------------------------------------------------------------------------------
// observe
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn observe_maps_facts_and_compacts_to_the_max() {
    let host = FakeHost::new();
    connected(&host);
    search_screen(&host);
    {
        let mut inner = host.inner();
        inner.foreground = Some(ForegroundApp {
            package_name: "com.example".to_string(),
            activity_name: Some("MainActivity".to_string()),
        });
    }
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 2);

    let screen = device.observe().await.expect("observation");

    assert_eq!(screen.width_px, 1080);
    assert_eq!(screen.height_px, 2400);
    assert_eq!(screen.package_name.as_deref(), Some("com.example"));
    assert_eq!(screen.activity_name.as_deref(), Some("MainActivity"));
    assert!(!screen.secure);
    assert_eq!(screen.nodes.len(), 2);

    let search = &screen.nodes[0];
    assert_eq!(search.id, 0);
    assert_eq!(search.description.as_deref(), Some("你要去哪儿"));
    assert_eq!(search.resource_id.as_deref(), Some("com.example:id/n"));
    assert_eq!(search.class_name.as_deref(), Some("EditText"));
    assert!(search.editable);
    assert_eq!(
        search.bounds,
        Bounds {
            left: 37,
            top: 100,
            right: 962,
            bottom: 150
        }
    );

    let button = &screen.nodes[1];
    assert_eq!(button.id, 1);
    assert_eq!(button.text.as_deref(), Some("快车"));
    assert!(button.clickable);

    // The root was not kept; the compactor never reached node 4 because the cap stopped it.
    assert_eq!(host.inner().releases, vec![1]);
}

#[tokio::test]
async fn observe_without_the_service_returns_the_kotlin_error() {
    let host = FakeHost::new();
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 200);

    let error = device.observe().await.expect_err("no service");

    assert_eq!(error.0, NOT_CONNECTED);
    assert_eq!(
        device.perform(tap(0)).await,
        ActionResult::failed(NOT_CONNECTED)
    );
}

#[tokio::test]
async fn screenshot_interval_is_enforced_through_the_clock() {
    let host = FakeHost::new();
    connected(&host);
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 200);

    // The first observation has no previous capture and never sleeps.
    device.observe().await.expect("first");
    assert!(clock.sleeps().is_empty());

    // 500 ms of unrelated work passed: only the rest of the interval is waited out.
    clock.set_now(10_500);
    device.observe().await.expect("second");
    assert_eq!(clock.sleeps(), vec![SCREENSHOT_INTERVAL_MS - 500]);

    // Nothing passed since the capture: the whole interval is waited out.
    device.observe().await.expect("third");
    assert_eq!(clock.sleeps(), vec![550, SCREENSHOT_INTERVAL_MS]);
    assert_eq!(host.inner().captures, 3);
}

#[tokio::test]
async fn secure_window_has_no_screenshot() {
    let host = FakeHost::new();
    connected(&host);
    {
        let mut inner = host.inner();
        inner.screenshot = ScreenShot {
            png: Some(vec![1, 2, 3]),
            secure: true,
        };
    }
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 200);

    let screen = device.observe().await.expect("observation");

    assert!(screen.secure);
    assert!(screen.screenshot_png.is_none());
}

#[tokio::test]
async fn all_black_capture_is_reported_as_secure() {
    let host = FakeHost::new();
    connected(&host);
    {
        let mut inner = host.inner();
        inner.display = (40, 40);
        inner.screenshot = ScreenShot {
            png: Some(png(40, 40, [0, 0, 0])),
            secure: false,
        };
    }
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 200);

    let screen = device.observe().await.expect("observation");

    assert!(screen.secure);
    assert!(screen.screenshot_png.is_none());
}

#[tokio::test]
async fn screenshot_is_downscaled_and_encoded_as_rgb_png() {
    let host = FakeHost::new();
    connected(&host);
    {
        let mut inner = host.inner();
        inner.display = (1600, 900);
        inner.screenshot = ScreenShot {
            png: Some(png(1600, 900, [200, 30, 30])),
            secure: false,
        };
    }
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 200);

    let screen = device.observe().await.expect("observation");

    assert!(!screen.secure);
    let encoded = screen.screenshot_png.expect("png bytes");
    let decoded = image::load_from_memory(&encoded).expect("decode");
    assert_eq!((decoded.width(), decoded.height()), (1280, 720));
    assert_eq!(
        decoded.to_rgb8().get_pixel(640, 360),
        &image::Rgb([200, 30, 30])
    );
}

// ---------------------------------------------------------------------------------------------
// perform
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn tap_uses_the_node_handle_and_falls_back_to_the_point() {
    let host = FakeHost::new();
    connected(&host);
    {
        let mut inner = host.inner();
        inner.roots = vec![1];
        inner.nodes.insert(
            1,
            FakeNode {
                bounds: (0, 0, 1080, 2400),
                children: vec![2, 3],
                ..FakeNode::default()
            },
        );
        inner.nodes.insert(
            2,
            FakeNode {
                text: Some("快车".to_string()),
                bounds: (0, 2160, 540, 2400),
                clickable: true,
                ..FakeNode::default()
            },
        );
        inner.nodes.insert(
            3,
            FakeNode {
                text: Some("标题".to_string()),
                bounds: (0, 0, 1080, 240),
                ..FakeNode::default()
            },
        );
    }
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 200);
    device.observe().await.expect("observation");

    // The clickable node goes through its handle.
    let result = device.perform(tap(0)).await;
    assert_eq!(result, ActionResult::ok("click node 0"));
    assert_eq!(host.inner().clicks, vec![2]);
    assert!(host.inner().strokes.is_empty());

    // The label is not clickable itself: the device presses its center instead.
    let result = device.perform(tap(1)).await;
    assert_eq!(result, ActionResult::ok("press node 1"));
    let inner = host.inner();
    assert_eq!(inner.strokes.len(), 1);
    assert_eq!(inner.strokes[0].points, vec![(540.0, 120.0)]);
    assert_eq!(inner.strokes[0].duration_ms, 60);
}

#[tokio::test]
async fn unknown_node_id_returns_the_kotlin_error() {
    let host = FakeHost::new();
    connected(&host);
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 200);
    device.observe().await.expect("observation");

    assert_eq!(
        device.perform(tap(7)).await,
        ActionResult::failed("unknown nodeId 7")
    );
    assert_eq!(
        device.perform(type_text(7, "北京", false)).await,
        ActionResult::failed("unknown nodeId 7")
    );
}

#[tokio::test]
async fn text_input_focuses_a_non_editable_node_then_types_and_submits() {
    let host = FakeHost::new();
    connected(&host);
    {
        let mut inner = host.inner();
        inner.roots = vec![1];
        inner.nodes.insert(
            1,
            FakeNode {
                bounds: (0, 0, 1080, 2400),
                children: vec![2],
                ..FakeNode::default()
            },
        );
        // A search bar placeholder: clickable, but the real input is the focused node.
        inner.nodes.insert(
            2,
            FakeNode {
                text: Some("搜索".to_string()),
                bounds: (40, 240, 1040, 360),
                clickable: true,
                ..FakeNode::default()
            },
        );
        inner.focus = Some(900);
    }
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 200);
    device.observe().await.expect("observation");

    let result = device.perform(type_text(0, "北京", true)).await;

    assert_eq!(result, ActionResult::ok("typed and submitted"));
    let inner = host.inner();
    assert_eq!(
        inner.clicks,
        vec![2],
        "the placeholder is tapped to open the input"
    );
    assert_eq!(inner.set_texts, vec![(900, "北京".to_string())]);
    assert_eq!(inner.ime_enters, vec![900]);
    assert_eq!(
        inner
            .releases
            .iter()
            .filter(|handle| **handle == 900)
            .count(),
        1
    );
    assert_eq!(clock.sleeps(), vec![400, 150]);
}

#[tokio::test]
async fn editable_node_is_typed_without_a_focus_tap() {
    let host = FakeHost::new();
    connected(&host);
    {
        let mut inner = host.inner();
        inner.roots = vec![1];
        inner.nodes.insert(
            1,
            FakeNode {
                bounds: (0, 0, 1080, 2400),
                children: vec![2],
                ..FakeNode::default()
            },
        );
        inner.nodes.insert(
            2,
            FakeNode {
                hint: Some("你要去哪儿".to_string()),
                bounds: (40, 240, 1040, 360),
                editable: true,
                ..FakeNode::default()
            },
        );
    }
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 200);
    device.observe().await.expect("observation");

    let result = device.perform(type_text(0, "北京", false)).await;

    assert_eq!(result, ActionResult::ok("typed"));
    let inner = host.inner();
    assert!(
        inner.clicks.is_empty(),
        "an editable node needs no focus tap"
    );
    assert_eq!(inner.set_texts, vec![(2, "北京".to_string())]);
    assert!(inner.ime_enters.is_empty());
    assert!(clock.sleeps().is_empty());
}

// ---------------------------------------------------------------------------------------------
// handle ownership
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn kept_handles_are_released_when_the_next_observation_replaces_them() {
    let host = FakeHost::new();
    connected(&host);
    search_screen(&host);
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 2);

    device.observe().await.expect("first");
    // Only the root is dropped: nodes 2 and 3 are kept.
    assert_eq!(host.inner().releases, vec![1]);
    assert!(
        host.inner()
            .releases
            .iter()
            .all(|handle| *handle != 2 && *handle != 3)
    );

    device.observe().await.expect("second");
    let releases = host.inner().releases.clone();
    assert_eq!(releases.iter().filter(|handle| **handle == 2).count(), 1);
    assert_eq!(releases.iter().filter(|handle| **handle == 3).count(), 1);
}

// ---------------------------------------------------------------------------------------------
// installed apps
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn installed_apps_are_mapped_deduped_and_sorted() {
    let host = FakeHost::new();
    {
        let mut inner = host.inner();
        inner.installed_json = serde_json::json!([
            {"packageName": "com.b", "label": "B 应用"},
            {"packageName": "com.a", "label": "A 应用"},
            {"packageName": "com.a", "label": "重复"},
            {"packageName": "com.c", "label": ""},
        ])
        .to_string();
    }
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 200);

    let apps = device.installed_apps().await;

    assert_eq!(
        apps,
        vec![
            AppInfo {
                package_name: "com.a".to_string(),
                label: "A 应用".to_string()
            },
            AppInfo {
                package_name: "com.b".to_string(),
                label: "B 应用".to_string()
            },
            AppInfo {
                package_name: "com.c".to_string(),
                label: "com.c".to_string()
            },
        ]
    );
}

// The point fallback of the action, without a node id.
#[tokio::test]
async fn tap_by_point_presses_the_denormalized_coordinate() {
    let host = FakeHost::new();
    connected(&host);
    let clock = FakeClock::new(10_000);
    let device = device(&host, &clock, 200);
    device.observe().await.expect("observation");

    let action = Action::Tap {
        point: Some(Point::new(500, 500).expect("point")),
        node_id: None,
        target: None,
        reason: None,
    };
    let result = device.perform(action).await;

    assert_eq!(result, ActionResult::ok("press (500,500)"));
    let inner = host.inner();
    assert_eq!(inner.strokes.len(), 1);
    assert_eq!(inner.strokes[0].points, vec![(540.0, 1200.0)]);
}
