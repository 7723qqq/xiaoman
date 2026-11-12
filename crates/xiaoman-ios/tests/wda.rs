//! Ports of `WdaDeviceTest` and `WdaSourceTest` against a wiremock WebDriverAgent.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use xiaoman_api::{Action, AppInfo, Bounds, Device, Point};
use xiaoman_ios::wda_client::WdaClient;
use xiaoman_ios::wda_device::SleepFn;
use xiaoman_ios::{WdaDevice, WdaDeviceOptions, WdaSource};

#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    path: String,
    body: Option<Value>,
    upgrade: bool,
}

struct FakeState {
    requests: Vec<Recorded>,
    xml: String,
    screenshot: String,
    sessions: u32,
    nested_session: bool,
    invalid_actions: u32,
    action_error: Option<String>,
    focused_elements: Value,
}

#[derive(Clone)]
struct FakeResponder(Arc<Mutex<FakeState>>);

impl Respond for FakeResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = self.0.lock().unwrap();
        let path = match request.url.query() {
            Some(query) => format!("{}?{}", request.url.path(), query),
            None => request.url.path().to_string(),
        };
        let body = if request.body.is_empty() {
            None
        } else {
            serde_json::from_slice(&request.body).ok()
        };
        state.requests.push(Recorded {
            method: request.method.to_string(),
            path: path.clone(),
            body,
            upgrade: request.headers.get("upgrade").is_some(),
        });

        let (status, value) = if path == "/session" {
            state.sessions += 1;
            let id = format!("s{}", state.sessions);
            if state.nested_session {
                (200, wrapped(json!({ "sessionId": id })))
            } else {
                (200, json!({ "sessionId": id, "value": {} }))
            }
        } else if path.ends_with("/actions") && state.invalid_actions > 0 {
            state.invalid_actions -= 1;
            (404, wrapped(json!({ "error": "invalid session id" })))
        } else if path.ends_with("/actions") && state.action_error.is_some() {
            (
                500,
                wrapped(json!({ "error": state.action_error.clone().unwrap() })),
            )
        } else if path.ends_with("/elements") {
            (200, wrapped(state.focused_elements.clone()))
        } else if path.ends_with("/window/size") {
            (200, wrapped(json!({ "width": 390, "height": 844 })))
        } else if path == "/screenshot" {
            (200, wrapped(Value::String(state.screenshot.clone())))
        } else if path == "/source?format=xml" {
            (200, wrapped(Value::String(state.xml.clone())))
        } else if path == "/wda/activeAppInfo" {
            (200, wrapped(json!({ "bundleId": "com.example.test" })))
        } else {
            (200, wrapped(Value::Null))
        };
        ResponseTemplate::new(status).set_body_json(value)
    }
}

struct FakeWda {
    server: MockServer,
    state: Arc<Mutex<FakeState>>,
}

impl FakeWda {
    async fn start(xml: &str, screenshot: String) -> Self {
        let state = Arc::new(Mutex::new(FakeState {
            requests: Vec::new(),
            xml: xml.to_string(),
            screenshot,
            sessions: 0,
            nested_session: false,
            invalid_actions: 0,
            action_error: None,
            focused_elements: json!([]),
        }));
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(FakeResponder(state.clone()))
            .mount(&server)
            .await;
        Self { server, state }
    }

    fn client(&self) -> WdaClient {
        WdaClient::with_timeout(self.server.uri(), Duration::from_secs(3)).unwrap()
    }

    fn update(&self, change: impl FnOnce(&mut FakeState)) {
        change(&mut self.state.lock().unwrap());
    }

    fn requests(&self) -> Vec<Recorded> {
        self.state.lock().unwrap().requests.clone()
    }

    fn actions(&self) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|request| request.path.ends_with("/actions"))
            .collect()
    }

    fn keys_requests(&self) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|request| request.path.ends_with("/wda/keys"))
            .collect()
    }

    fn sessions(&self) -> u32 {
        self.state.lock().unwrap().sessions
    }
}

fn device(fake: &FakeWda, options: WdaDeviceOptions) -> WdaDevice {
    WdaDevice::with_options(Arc::new(fake.client()), options).unwrap()
}

fn options(settle_millis: u64) -> (WdaDeviceOptions, Arc<Mutex<Vec<u64>>>) {
    let waits = Arc::new(Mutex::new(Vec::new()));
    let sink = waits.clone();
    let sleep: SleepFn = Arc::new(move |millis| {
        let sink = sink.clone();
        let future: Pin<Box<dyn Future<Output = ()> + Send>> =
            Box::pin(async move { sink.lock().unwrap().push(millis) });
        future
    });
    (
        WdaDeviceOptions {
            settle_millis,
            sleep,
            ..Default::default()
        },
        waits,
    )
}

fn wrapped(value: Value) -> Value {
    json!({ "value": value })
}

fn png_base64(width: u32, height: u32) -> String {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height));
    let mut cursor = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut cursor, image::ImageFormat::Png)
        .unwrap();
    base64::engine::general_purpose::STANDARD.encode(cursor.into_inner())
}

fn base64_text(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
}

fn steps(request: &Recorded) -> Vec<Value> {
    request.body.as_ref().unwrap()["actions"][0]["actions"]
        .as_array()
        .unwrap()
        .clone()
}

fn assert_move(step: &Value, x: i32, y: i32, duration: u64) {
    assert_eq!(step["type"], json!("pointerMove"));
    assert_eq!(step["x"], json!(x));
    assert_eq!(step["y"], json!(y));
    assert_eq!(step["duration"], json!(duration));
}

fn key_values(request: &Recorded) -> Vec<String> {
    request.body.as_ref().unwrap()["value"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect()
}

fn paths(requests: &[Recorded]) -> Vec<String> {
    requests
        .iter()
        .map(|request| request.path.clone())
        .collect()
}

fn tap_node(node_id: i32) -> Action {
    Action::Tap {
        node_id: Some(node_id),
        point: None,
        target: None,
        reason: None,
    }
}

fn tap_point(x: i32, y: i32) -> Action {
    Action::Tap {
        node_id: None,
        point: Some(Point::new(x, y).unwrap()),
        target: None,
        reason: None,
    }
}

fn type_text(text: &str, node_id: Option<i32>, submit: bool) -> Action {
    Action::TypeText {
        text: text.to_string(),
        node_id,
        submit,
        target: None,
        reason: None,
    }
}

const SOURCE_XML: &str = r#"<root>
  <XCUIElementTypeNavigationBar visible="true" x="0" y="40" width="390" height="60">
    <XCUIElementTypeButton visible="true" enabled="true" label="返回" x="10" y="50" width="60" height="40"/>
  </XCUIElementTypeNavigationBar>
  <XCUIElementTypeTextField visible="true" enabled="true" label="姓名" value="旧内容" x="20" y="150" width="200" height="40"/>
  <XCUIElementTypeTextField visible="true" enabled="true" label="搜索" value="搜索" placeholderValue="搜索" x="20" y="200" width="200" height="40"/>
</root>"#;

#[tokio::test]
async fn observe_scales_png_and_uses_window_points_and_bundle_id() {
    let fake = FakeWda::start(SOURCE_XML, png_base64(780, 1688)).await;
    let (options, _waits) = options(0);
    let device = device(
        &fake,
        WdaDeviceOptions {
            screenshot_max_side: 400,
            ..options
        },
    );
    assert_eq!(fake.sessions(), 0);

    let screen = device.observe().await.unwrap();
    assert_eq!(fake.sessions(), 1);
    assert_eq!(screen.package_name.as_deref(), Some("com.example.test"));
    assert_eq!(screen.width_px, 390);
    assert_eq!(screen.height_px, 844);
    assert_eq!(screen.activity_name, None);
    assert!(!screen.secure);
    let png = screen.screenshot_png.clone().expect("screenshot");
    assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
    let image = image::load_from_memory(&png).unwrap();
    assert_eq!(image.height(), 400);
    assert_eq!(image.width(), 185);
    assert_eq!(image.color(), image::ColorType::Rgb8);
    assert_eq!(screen.nodes.first().unwrap().text.as_deref(), Some("返回"));
    assert_eq!(
        screen.nodes.first().unwrap().bounds,
        Bounds {
            left: 10,
            top: 50,
            right: 70,
            bottom: 90
        }
    );

    fake.update(|state| state.screenshot = "not base64".to_string());
    assert!(device.observe().await.unwrap().screenshot_png.is_none());
    fake.update(|state| state.screenshot = base64_text("not an image"));
    assert!(device.observe().await.unwrap().screenshot_png.is_none());
    fake.update(|state| state.screenshot = png_base64(20, 40));
    let small =
        image::load_from_memory(&device.observe().await.unwrap().screenshot_png.unwrap()).unwrap();
    assert_eq!(small.width(), 20);
    assert_eq!(small.height(), 40);
    assert_eq!(fake.sessions(), 1);
}

#[tokio::test]
async fn taps_long_press_and_swipe_produce_w3c_point_actions_and_settle() {
    let fake = FakeWda::start(SOURCE_XML, png_base64(780, 1688)).await;
    let (options, waits) = options(600);
    let device = device(&fake, options);
    device.observe().await.unwrap();

    assert!(device.perform(tap_node(0)).await.ok);
    assert_move(&steps(fake.actions().last().unwrap())[0], 40, 70, 0);
    assert!(device.perform(tap_point(500, 250)).await.ok);
    assert_move(&steps(fake.actions().last().unwrap())[0], 195, 211, 0);
    assert!(device.perform(tap_point(1000, 1000)).await.ok);
    assert_move(&steps(fake.actions().last().unwrap())[0], 389, 843, 0);

    let long_press = Action::LongPress {
        node_id: Some(0),
        point: None,
        duration_ms: 1250,
        target: None,
        reason: None,
    };
    assert!(device.perform(long_press).await.ok);
    assert_eq!(
        steps(fake.actions().last().unwrap())[2]["duration"],
        json!(1250)
    );

    let swipe = Action::Swipe {
        from: Point::new(100, 200).unwrap(),
        to: Point::new(900, 800).unwrap(),
        duration_ms: 720,
        target: None,
        reason: None,
    };
    assert!(device.perform(swipe).await.ok);
    let recorded = fake.actions();
    let request = recorded.last().unwrap();
    let pointer = request.body.as_ref().unwrap()["actions"][0].clone();
    assert_eq!(pointer["type"], json!("pointer"));
    assert_eq!(pointer["id"], json!("finger1"));
    assert_eq!(pointer["parameters"]["pointerType"], json!("touch"));
    let actions = steps(request);
    assert_eq!(
        actions
            .iter()
            .map(|step| step["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "pointerMove",
            "pointerDown",
            "pause",
            "pointerMove",
            "pointerUp"
        ]
    );
    assert_move(&actions[0], 39, 169, 0);
    assert_move(&actions[3], 351, 675, 720);
    assert_eq!(actions[1]["button"], json!(0));
    assert_eq!(actions[4]["button"], json!(0));
    assert_eq!(*waits.lock().unwrap(), vec![600, 600, 600, 600, 600]);
}

#[tokio::test]
async fn typing_focuses_waits_clears_sets_value_and_submits() {
    let fake = FakeWda::start(SOURCE_XML, png_base64(780, 1688)).await;
    fake.update(|state| state.focused_elements = json!([{ "ELEMENT": "field1" }]));
    let (options, waits) = options(600);
    let device = device(&fake, options);
    device.observe().await.unwrap();

    let before = fake.requests().len();
    assert!(device.perform(type_text("你好😀", Some(1), true)).await.ok);
    let sent: Vec<Recorded> = fake.requests().into_iter().skip(before).collect();
    assert_eq!(
        paths(&sent),
        vec![
            "/session/s1/actions",
            "/session/s1/elements",
            "/session/s1/element/field1/clear",
            "/session/s1/element/field1/value",
            "/session/s1/wda/keys",
        ]
    );
    assert!(sent.iter().all(|request| request.method == "POST"));
    assert_move(&steps(&sent[0])[0], 120, 170, 0);
    assert_eq!(
        sent[1].body,
        Some(json!({ "using": "predicate string", "value": "hasKeyboardFocus == 1" }))
    );
    assert_eq!(key_values(&sent[3]), vec!["你", "好", "😀"]);
    assert_eq!(key_values(&sent[4]), vec!["\n"]);
    assert_eq!(*waits.lock().unwrap(), vec![500, 600]);
}

#[tokio::test]
async fn typing_placeholder_uses_w3c_element_without_clearing() {
    let fake = FakeWda::start(SOURCE_XML, png_base64(780, 1688)).await;
    fake.update(|state| {
        state.focused_elements = json!([{ "element-6066-11e4-a52e-4f735466cecf": "field2" }])
    });
    let (options, waits) = options(100);
    let device = device(&fake, options);
    device.observe().await.unwrap();

    let before = fake.requests().len();
    assert!(device.perform(type_text("新值", Some(2), false)).await.ok);
    let sent: Vec<Recorded> = fake.requests().into_iter().skip(before).collect();
    assert_eq!(
        paths(&sent),
        vec![
            "/session/s1/actions",
            "/session/s1/elements",
            "/session/s1/element/field2/value"
        ]
    );
    assert_eq!(key_values(sent.last().unwrap()), vec!["新", "值"]);
    assert_eq!(*waits.lock().unwrap(), vec![100, 100]);

    let next = fake.requests().len();
    assert!(device.perform(type_text("焦点", None, false)).await.ok);
    assert_eq!(
        paths(&fake.requests().into_iter().skip(next).collect::<Vec<_>>()),
        vec!["/session/s1/elements", "/session/s1/element/field2/value"]
    );
    assert_eq!(*waits.lock().unwrap(), vec![100, 100, 100]);
}

#[tokio::test]
async fn typing_without_focused_element_uses_keys_and_reports_uncleared_content() {
    let fake = FakeWda::start(SOURCE_XML, png_base64(780, 1688)).await;
    let (options, waits) = options(120);
    let device = device(&fake, options);
    device.observe().await.unwrap();

    let before = fake.requests().len();
    let result = device.perform(type_text("你好😀", Some(1), true)).await;
    assert!(result.ok);
    assert!(
        result
            .message
            .unwrap()
            .contains("未能定位输入框，原有内容未清空")
    );
    let sent: Vec<Recorded> = fake.requests().into_iter().skip(before).collect();
    assert_eq!(
        paths(&sent),
        vec![
            "/session/s1/actions",
            "/session/s1/elements",
            "/session/s1/wda/keys"
        ]
    );
    assert_eq!(
        key_values(sent.last().unwrap()),
        vec!["你", "好", "😀", "\n"]
    );
    assert_eq!(*waits.lock().unwrap(), vec![120, 120]);

    let next = fake.requests().len();
    let focused = device.perform(type_text("焦点", None, false)).await;
    assert!(focused.ok);
    assert!(focused.message.unwrap().contains("未清空"));
    assert_eq!(
        paths(&fake.requests().into_iter().skip(next).collect::<Vec<_>>()),
        vec!["/session/s1/elements", "/session/s1/wda/keys"]
    );
    assert_eq!(
        key_values(fake.keys_requests().last().unwrap()),
        vec!["焦", "点"]
    );
}

#[tokio::test]
async fn focused_element_accepts_both_id_keys_and_only_uses_first_match() {
    let fake = FakeWda::start(SOURCE_XML, png_base64(780, 1688)).await;
    let client = fake.client();
    assert_eq!(client.focused_element().await.unwrap(), None);
    for key in ["ELEMENT", "element-6066-11e4-a52e-4f735466cecf"] {
        let with_key = |value: &str| {
            let mut element = serde_json::Map::new();
            element.insert(key.to_string(), json!(value));
            Value::Object(element)
        };
        fake.update(|state| {
            state.focused_elements = json!([with_key("first"), with_key("second")])
        });
        assert_eq!(
            client.focused_element().await.unwrap(),
            Some("first".to_string())
        );
    }
    fake.update(|state| state.focused_elements = json!([{}, { "ELEMENT": "second" }]));
    assert_eq!(client.focused_element().await.unwrap(), None);
    assert_eq!(fake.sessions(), 1);
}

#[tokio::test]
async fn back_uses_navigation_button_then_falls_back_to_edge_swipe() {
    let fake = FakeWda::start(SOURCE_XML, png_base64(780, 1688)).await;
    let (options, _waits) = options(0);
    let device = device(&fake, options);
    device.observe().await.unwrap();

    let back = Action::Back {
        target: None,
        reason: None,
    };
    let result = device.perform(back.clone()).await;
    assert!(result.message.unwrap().contains("导航"));
    assert_move(&steps(fake.actions().last().unwrap())[0], 40, 70, 0);

    let with_top_button = SOURCE_XML.replace(
        "<root>",
        r#"<root><XCUIElementTypeButton visible="true" enabled="true" label="其他顶部按钮" x="0" y="0" width="20" height="20"/>"#,
    );
    fake.update(|state| state.xml = with_top_button);
    device.observe().await.unwrap();
    let result = device.perform(back.clone()).await;
    assert!(result.message.unwrap().contains("导航"));
    assert_move(&steps(fake.actions().last().unwrap())[0], 40, 70, 0);

    fake.update(|state| {
        state.xml =
            r#"<root><XCUIElementTypeButton visible="true" enabled="true" label="返回" x="10" y="20" width="40" height="30"/></root>"#
                .to_string()
    });
    device.observe().await.unwrap();
    let result = device.perform(back.clone()).await;
    assert!(result.message.unwrap().contains("按钮"));
    assert_move(&steps(fake.actions().last().unwrap())[0], 30, 35, 0);

    fake.update(|state| state.xml = "<root/>".to_string());
    device.observe().await.unwrap();
    let result = device.perform(back).await;
    assert!(result.message.unwrap().contains("左边缘"));
    let actions = steps(fake.actions().last().unwrap());
    assert_move(&actions[0], 1, 422, 0);
    assert_move(&actions[3], 273, 422, 300);
}

#[tokio::test]
async fn home_launch_deeplink_wait_and_loop_actions_use_expected_endpoints() {
    let fake = FakeWda::start(SOURCE_XML, png_base64(780, 1688)).await;
    let (options, waits) = options(12);
    let apps = vec![AppInfo {
        package_name: "com.custom".to_string(),
        label: "自定义".to_string(),
    }];
    let device = device(
        &fake,
        WdaDeviceOptions {
            apps: apps.clone(),
            ..options
        },
    );
    assert_eq!(device.installed_apps().await, apps);

    assert!(
        device
            .perform(Action::Home {
                target: None,
                reason: None
            })
            .await
            .ok
    );
    assert_eq!(fake.requests().last().unwrap().path, "/wda/homescreen");
    assert!(
        device
            .perform(Action::LaunchApp {
                package_name: "com.apple.Preferences".to_string(),
                target: None,
                reason: None,
            })
            .await
            .ok
    );
    let recorded = fake.requests();
    let launch = recorded.last().unwrap();
    assert_eq!(launch.path, "/session/s1/wda/apps/launch");
    assert_eq!(
        launch.body.as_ref().unwrap()["bundleId"],
        json!("com.apple.Preferences")
    );
    assert!(
        device
            .perform(Action::OpenDeeplink {
                uri: "https://example.test/你好".to_string(),
                package_name: None,
                target: None,
                reason: None,
            })
            .await
            .ok
    );
    let recorded = fake.requests();
    let deeplink = recorded.last().unwrap();
    assert_eq!(deeplink.path, "/session/s1/url");
    assert_eq!(
        deeplink.body.as_ref().unwrap()["url"],
        json!("https://example.test/你好")
    );

    let count = fake.requests().len();
    assert!(
        device
            .perform(Action::Wait {
                millis: 20_000,
                target: None,
                reason: None
            })
            .await
            .ok
    );
    assert!(
        device
            .perform(Action::Wait {
                millis: -10,
                target: None,
                reason: None
            })
            .await
            .ok
    );
    for action in [
        Action::AskUser {
            question: "需要登录".to_string(),
            takeover: false,
            target: None,
            reason: None,
        },
        Action::Finish {
            success: true,
            summary: "完成".to_string(),
            target: None,
            reason: None,
        },
    ] {
        let result = device.perform(action).await;
        assert!(!result.ok);
        assert!(result.message.unwrap().contains("循环"));
    }
    let mut unknown = tap_node(999);
    if let Action::Tap { point, .. } = &mut unknown {
        *point = Some(Point::new(1, 1).unwrap());
    }
    assert!(!device.perform(unknown).await.ok);
    assert!(
        !device
            .perform(Action::LongPress {
                node_id: Some(999),
                point: None,
                duration_ms: 800,
                target: None,
                reason: None,
            })
            .await
            .ok
    );
    assert!(!device.perform(type_text("秘密", Some(999), false)).await.ok);
    assert_eq!(fake.requests().len(), count);
    assert_eq!(*waits.lock().unwrap(), vec![12, 12, 12, 10_000, 0]);
    assert!(
        fake.requests()
            .iter()
            .filter(|request| request.path != "/session")
            .all(|request| request.method == "POST")
    );
}

#[tokio::test]
async fn invalid_session_rebuilds_once_and_other_failures_become_results() {
    let fake = FakeWda::start(SOURCE_XML, png_base64(780, 1688)).await;
    fake.update(|state| state.nested_session = true);
    let (options, _waits) = options(0);
    let device = device(&fake, options);
    device.observe().await.unwrap();

    fake.update(|state| state.invalid_actions = 1);
    assert!(device.perform(tap_node(0)).await.ok);
    assert_eq!(fake.sessions(), 2);
    assert_eq!(
        paths(&fake.actions()),
        vec!["/session/s1/actions", "/session/s2/actions"]
    );
    let actions = fake.actions();
    assert_eq!(actions[0].body, actions[1].body);
    let session_body = fake
        .requests()
        .into_iter()
        .find(|request| request.path == "/session")
        .unwrap()
        .body
        .unwrap();
    assert_eq!(session_body["capabilities"]["alwaysMatch"], json!({}));

    fake.update(|state| state.invalid_actions = 2);
    let failed = device.perform(tap_node(0)).await;
    assert!(!failed.ok);
    assert!(failed.message.unwrap().contains("404"));
    assert_eq!(fake.sessions(), 3);
    assert_eq!(fake.actions().len(), 4);

    fake.update(|state| state.action_error = Some("unknown error".to_string()));
    let other_failure = device.perform(tap_node(0)).await;
    assert!(!other_failure.ok);
    let message = other_failure.message.unwrap();
    assert!(message.contains("/session/s3/actions"));
    assert!(message.contains("500"));
    assert_eq!(fake.sessions(), 3);
}

#[tokio::test]
async fn point_action_before_observation_fetches_size_and_client_status_works() {
    let fake = FakeWda::start(SOURCE_XML, png_base64(780, 1688)).await;
    let (options, _waits) = options(0);
    let device = device(&fake, options);

    fake.client().status().await.unwrap();
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/status");
    assert!(!requests[0].upgrade);
    assert_eq!(fake.sessions(), 0);

    assert!(device.perform(tap_point(500, 500)).await.ok);
    assert_move(&steps(&fake.actions()[0])[0], 195, 422, 0);
    assert_eq!(fake.sessions(), 1);
}

#[tokio::test]
async fn transport_failure_is_reported_without_a_status() {
    let client = WdaClient::with_timeout("http://127.0.0.1:1", Duration::from_millis(500)).unwrap();
    let error = client.status().await.unwrap_err();
    assert_eq!(error.status_code, 0);
    assert_eq!(error.error.as_deref(), Some("transport failure"));
    assert!(error.to_string().contains("/status"));
}

#[test]
fn parses_document_order_point_bounds_and_input_metadata() {
    let xml = r#"<AppiumAUT>
  <XCUIElementTypeApplication visible="true" label="应用" x="0" y="0" width="390" height="844">
    <XCUIElementTypeWindow visible="true" name="窗口" x="0" y="0" width="390" height="844">
      <XCUIElementTypeNavigationBar visible="true" label="导航" x="0" y="40" width="390" height="50">
        <XCUIElementTypeButton visible="true" enabled="true" label="返回" name="back" x="-10" y="40" width="70" height="40"/>
      </XCUIElementTypeNavigationBar>
      <XCUIElementTypeStaticText visible="true" label="你好，世界" name="title" x="20.5" y="110.2" width="100" height="20"/>
      <XCUIElementTypeTextField visible="true" enabled="true" value="请输入姓名" placeholderValue="请输入姓名" x="20" y="160" width="200" height="40"/>
      <XCUIElementTypeSecureTextField visible="true" enabled="true" label="密码" value="••••" x="20" y="210" width="200" height="40"/>
      <XCUIElementTypeCell visible="true" enabled="true" x="0" y="260" width="400" height="50"/>
      <XCUIElementTypeButton visible="true" enabled="false" label="禁用" x="0" y="310" width="100" height="40"/>
      <XCUIElementTypeScrollView visible="true" x="0" y="360" width="390" height="500"/>
      <XCUIElementTypeStaticText visible="false" label="隐藏" x="0" y="0" width="100" height="40"/>
      <XCUIElementTypeButton visible="true" enabled="true" label="零尺寸" x="0" y="0" width="0" height="40"/>
      <XCUIElementTypeOther visible="true" x="0" y="0" width="100" height="40"/>
      <XCUIElementTypeStaticText visible="true" value="仅有值" x="0" y="800" width="100" height="100"/>
      <XCUIElementTypeButton visible="true" enabled="true" label="屏幕外" x="500" y="0" width="10" height="10"/>
    </XCUIElementTypeWindow>
  </XCUIElementTypeApplication>
</AppiumAUT>"#;
    let snapshot = WdaSource::parse_details(xml, 390, 844).unwrap();
    let nodes = WdaSource::parse(xml, 390, 844).unwrap();
    assert_eq!(
        nodes.iter().map(|node| node.id).collect::<Vec<_>>(),
        (0..9).collect::<Vec<_>>()
    );
    assert_eq!(
        nodes
            .iter()
            .map(|node| node.class_name.clone().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "NavigationBar",
            "Button",
            "StaticText",
            "TextField",
            "SecureTextField",
            "Cell",
            "Button",
            "ScrollView",
            "StaticText"
        ]
    );
    assert_eq!(
        nodes[1].bounds,
        Bounds {
            left: 0,
            top: 40,
            right: 60,
            bottom: 80
        }
    );
    assert_eq!(
        nodes[2].bounds,
        Bounds {
            left: 20,
            top: 110,
            right: 121,
            bottom: 131
        }
    );
    assert_eq!(nodes[2].text.as_deref(), Some("你好，世界"));
    assert_eq!(nodes[2].description.as_deref(), Some("title"));
    assert_eq!(nodes[1].description.as_deref(), Some("back"));
    assert_eq!(nodes[3].text, None);
    assert_eq!(nodes[3].description.as_deref(), Some("请输入姓名"));
    assert!(nodes[3].editable && nodes[3].clickable);
    assert!(nodes[4].password && nodes[4].editable && nodes[4].clickable);
    assert!(nodes[5].clickable);
    assert!(!nodes[6].clickable);
    assert!(nodes[7].scrollable);
    assert_eq!(
        nodes[7].bounds,
        Bounds {
            left: 0,
            top: 360,
            right: 390,
            bottom: 844
        }
    );
    assert_eq!(nodes[8].text.as_deref(), Some("仅有值"));
    assert_eq!(nodes[8].bounds.bottom, 844);
    assert!(nodes.iter().all(|node| node.resource_id.is_none()));
    assert_eq!(snapshot.entries[4].node_type, "SecureTextField");
    assert_eq!(snapshot.entries[4].value.as_deref(), Some("••••"));
    assert_eq!(
        snapshot.entries[3].placeholder.as_deref(),
        Some("请输入姓名")
    );
    assert_eq!(
        snapshot.navigation_bars,
        vec![Bounds {
            left: 0,
            top: 40,
            right: 390,
            bottom: 90
        }]
    );
}

#[test]
fn flags_cover_all_interactive_and_scrollable_types() {
    let types = [
        "Link",
        "Switch",
        "Key",
        "Icon",
        "Image",
        "SearchField",
        "TextView",
        "Table",
        "CollectionView",
        "WebView",
    ];
    let mut xml = String::from("<root>");
    for node_type in types {
        xml.push_str(&format!(
            r#"<XCUIElementType{node_type} visible="true" enabled="true" label="内容" x="0" y="0" width="20" height="20"/>"#
        ));
    }
    xml.push_str("</root>");
    let nodes = WdaSource::parse(&xml, 390, 844).unwrap();
    assert!(nodes.iter().take(7).all(|node| node.clickable));
    assert!(nodes[5..=6].iter().all(|node| node.editable));
    assert!(
        nodes[7..]
            .iter()
            .all(|node| node.scrollable && !node.clickable)
    );
}

#[test]
fn empty_screen_and_external_entities_are_rejected() {
    assert_eq!(WdaSource::parse("<root/>", 0, 844).unwrap(), vec![]);
    assert!(
        WdaSource::parse(
            r#"<!DOCTYPE root [<!ENTITY x SYSTEM "file:///not-authorized">]><root>&x;</root>"#,
            390,
            844
        )
        .is_err()
    );
}
