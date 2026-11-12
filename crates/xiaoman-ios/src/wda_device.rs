//! The iPhone hands and eyes: one [`Device`] over a running WebDriverAgent.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use xiaoman_api::{Action, ActionResult, AppInfo, Bounds, Device, DeviceError, Point, ScreenState};

use crate::ios_platform::IosPlatform;
use crate::wda_client::{WdaClient, WdaError, json_content};
use crate::wda_source::{Snapshot, WdaSource, WdaSourceError};

/// How the device waits between actions; replace it in tests to avoid real sleeping.
pub type SleepFn = Arc<dyn Fn(u64) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// [`WdaDevice`] settings, mirroring the Kotlin constructor parameters.
#[derive(Clone)]
pub struct WdaDeviceOptions {
    /// Apps offered to the operator model.
    pub apps: Vec<AppInfo>,
    /// Long edge of the screenshot sent to the model, in pixels.
    pub screenshot_max_side: u32,
    /// Pause after a successful action so the screen settles, in milliseconds.
    pub settle_millis: u64,
    pub sleep: SleepFn,
}

impl Default for WdaDeviceOptions {
    fn default() -> Self {
        Self {
            apps: IosPlatform::default_apps(),
            screenshot_max_side: 1280,
            settle_millis: 600,
            sleep: Arc::new(|millis| {
                Box::pin(tokio::time::sleep(std::time::Duration::from_millis(millis)))
            }),
        }
    }
}

#[derive(Default)]
struct State {
    width: i32,
    height: i32,
    latest: Snapshot,
}

/// iPhone hands and eyes backed by a running WebDriverAgent.
pub struct WdaDevice {
    client: Arc<WdaClient>,
    apps: Vec<AppInfo>,
    screenshot_max_side: u32,
    settle_millis: u64,
    sleep: SleepFn,
    state: Mutex<State>,
}

impl WdaDevice {
    /// Device with the default apps, 1280 px screenshots and a 600 ms settle pause.
    pub fn new(client: Arc<WdaClient>) -> Result<Self, DeviceError> {
        Self::with_options(client, WdaDeviceOptions::default())
    }

    pub fn with_options(
        client: Arc<WdaClient>,
        options: WdaDeviceOptions,
    ) -> Result<Self, DeviceError> {
        if options.screenshot_max_side == 0 {
            return Err(DeviceError(
                "screenshot_max_side must be positive".to_string(),
            ));
        }
        Ok(Self {
            client,
            apps: options.apps,
            screenshot_max_side: options.screenshot_max_side,
            settle_millis: options.settle_millis,
            sleep: options.sleep,
            state: Mutex::new(State::default()),
        })
    }

    pub fn client(&self) -> &Arc<WdaClient> {
        &self.client
    }

    async fn read_size(&self) -> Result<(i32, i32), WdaError> {
        let size = self.client.window_size().await?;
        let dimension = |key: &str| -> Result<i32, WdaError> {
            let value = size.get(key).and_then(json_number);
            value
                .filter(|value| value.is_finite() && *value > 0.0 && *value <= f64::from(i32::MAX))
                .map(|value| value.round() as i32)
                .filter(|value| *value > 0)
                .ok_or_else(|| WdaError::new("/window/size", 200, Some("invalid window size")))
        };
        Ok((dimension("width")?, dimension("height")?))
    }

    async fn ensure_size(&self, state: &mut State) -> Result<(), WdaError> {
        if state.width <= 0 || state.height <= 0 {
            let (width, height) = self.read_size().await?;
            state.width = width;
            state.height = height;
        }
        Ok(())
    }

    fn coordinates(&self, state: &State, point: Point) -> (i32, i32) {
        let x = f64::from(point.x()) * f64::from(state.width) / f64::from(Point::NORM);
        let y = f64::from(point.y()) * f64::from(state.height) / f64::from(Point::NORM);
        (
            clamp_x(x.round() as i32, state.width),
            clamp_y(y.round() as i32, state.height),
        )
    }

    fn center(&self, state: &State, bounds: &Bounds) -> (i32, i32) {
        let x = (i64::from(bounds.left) + i64::from(bounds.right)) / 2;
        let y = (i64::from(bounds.top) + i64::from(bounds.bottom)) / 2;
        (
            clamp_x(x as i32, state.width),
            clamp_y(y as i32, state.height),
        )
    }

    /// One W3C pointer gesture: move to `from`, press, optional move to `to`, release.
    async fn gesture(
        &self,
        from: (i32, i32),
        to: Option<(i32, i32)>,
        duration: u64,
    ) -> Result<(), WdaError> {
        let mut steps = vec![
            json!({ "type": "pointerMove", "duration": 0, "x": from.0, "y": from.1 }),
            json!({ "type": "pointerDown", "button": 0 }),
            json!({ "type": "pause", "duration": if to.is_none() { duration } else { 0 } }),
        ];
        if let Some(to) = to {
            steps
                .push(json!({ "type": "pointerMove", "duration": duration, "x": to.0, "y": to.1 }));
        }
        steps.push(json!({ "type": "pointerUp", "button": 0 }));
        let body = json!({
            "actions": [{
                "type": "pointer",
                "id": "finger1",
                "parameters": { "pointerType": "touch" },
                "actions": steps,
            }]
        });
        self.client.actions(body).await
    }

    async fn press(
        &self,
        state: &mut State,
        node_id: Option<i32>,
        point: Option<Point>,
        duration: u64,
    ) -> Result<ActionResult, WdaError> {
        let position = if let Some(node_id) = node_id {
            match lookup(state, node_id) {
                Some(entry) => self.center(state, &entry.ui.bounds),
                None => return Ok(unknown_node(node_id)),
            }
        } else {
            let Some(point) = point else {
                return Ok(ActionResult::failed("点击需要 nodeId 或 point"));
            };
            self.ensure_size(state).await?;
            self.coordinates(state, point)
        };
        self.gesture(position, None, duration).await?;
        Ok(ActionResult::ok("已点击"))
    }

    async fn type_text(
        &self,
        state: &mut State,
        action: &Action,
    ) -> Result<ActionResult, WdaError> {
        let Action::TypeText {
            text,
            node_id,
            submit,
            ..
        } = action
        else {
            return Ok(ActionResult::failed("此动作由操作循环处理"));
        };
        let (entry, typed_into_node) = match node_id {
            Some(node_id) => match lookup(state, *node_id) {
                Some(entry) => (Some(entry), true),
                None => return Ok(unknown_node(*node_id)),
            },
            None => (
                state
                    .latest
                    .entries
                    .iter()
                    .find(|entry| entry.ui.editable && entry.ui.focused),
                false,
            ),
        };
        if typed_into_node {
            if let Some(entry) = entry {
                let center = self.center(state, &entry.ui.bounds);
                self.gesture(center, None, 60).await?;
                (self.sleep)(self.settle_millis.min(500)).await;
            }
        }
        let old_value = entry.and_then(|entry| match (&entry.value, &entry.placeholder) {
            (Some(value), placeholder) if Some(value) != placeholder.as_ref() => {
                Some(value.clone())
            }
            _ => None,
        });
        let Some(element_id) = self.client.focused_element().await? else {
            let input = if *submit {
                format!("{text}\n")
            } else {
                text.clone()
            };
            let keys: Vec<String> = input
                .chars()
                .map(|character| character.to_string())
                .collect();
            self.client.keys(&keys).await?;
            let done = if *submit {
                "已输入并提交"
            } else {
                "已输入"
            };
            return Ok(ActionResult::ok(format!(
                "未能定位输入框，原有内容未清空；{done}"
            )));
        };
        if old_value.as_deref().is_some_and(|old| !old.is_empty()) {
            self.client.clear(&element_id).await?;
        }
        self.client.set_value(&element_id, text).await?;
        if *submit {
            self.client.keys(&["\n".to_string()]).await?;
        }
        Ok(ActionResult::ok(if *submit {
            "已输入并提交"
        } else {
            "已输入"
        }))
    }

    async fn back(&self, state: &mut State) -> Result<ActionResult, WdaError> {
        self.ensure_size(state).await?;
        let width = state.width;
        let height = state.height;
        let buttons: Vec<&crate::wda_source::Entry> = state
            .latest
            .entries
            .iter()
            .filter(|entry| entry.ui.class_name.as_deref() == Some("Button") && entry.ui.clickable)
            .collect();
        let bars = &state.latest.navigation_bars;
        let in_navigation_bar = |bounds: &Bounds| {
            bars.iter().any(|bar| {
                bounds.left >= bar.left
                    && bounds.top >= bar.top
                    && bounds.right <= bar.right
                    && bounds.bottom <= bar.bottom
                    && (bounds.left + bounds.right) / 2 <= bar.left + (bar.right - bar.left) / 3
            })
        };
        let button = buttons
            .iter()
            .copied()
            .find(|entry| in_navigation_bar(&entry.ui.bounds))
            .or_else(|| {
                buttons.iter().copied().find(|entry| {
                    let bounds = &entry.ui.bounds;
                    f64::from(bounds.top) < f64::from(height) * 0.15
                        && f64::from((bounds.left + bounds.right) / 2) < f64::from(width) / 3.0
                })
            });
        if let Some(entry) = button {
            let center = self.center(state, &entry.ui.bounds);
            self.gesture(center, None, 60).await?;
            return Ok(ActionResult::ok("已点击左上角导航返回按钮"));
        }
        let from = (1, height / 2);
        let to = ((f64::from(width) * 0.7).round() as i32, height / 2);
        self.gesture(from, Some(to), 300).await?;
        Ok(ActionResult::ok("已从左边缘向右滑动返回"))
    }

    async fn execute(&self, state: &mut State, action: &Action) -> Result<ActionResult, WdaError> {
        match action {
            Action::Tap { node_id, point, .. } => self.press(state, *node_id, *point, 60).await,
            Action::LongPress {
                node_id,
                point,
                duration_ms,
                ..
            } => {
                self.press(state, *node_id, *point, (*duration_ms).max(0) as u64)
                    .await
            }
            Action::Swipe {
                from,
                to,
                duration_ms,
                ..
            } => {
                self.ensure_size(state).await?;
                let from = self.coordinates(state, *from);
                let to = self.coordinates(state, *to);
                self.gesture(from, Some(to), (*duration_ms).max(0) as u64)
                    .await?;
                Ok(ActionResult::ok("已滑动"))
            }
            Action::TypeText { .. } => self.type_text(state, action).await,
            Action::Back { .. } => self.back(state).await,
            Action::Home { .. } => {
                self.client.home().await?;
                Ok(ActionResult::ok("已回到主屏幕"))
            }
            Action::LaunchApp { package_name, .. } => {
                self.client.launch_app(package_name).await?;
                Ok(ActionResult::ok("已启动应用"))
            }
            Action::OpenDeeplink { uri, .. } => {
                self.client.open_deeplink(uri).await?;
                Ok(ActionResult::ok("已打开链接"))
            }
            Action::Wait { millis, .. } => {
                (self.sleep)((*millis).clamp(0, 10_000) as u64).await;
                Ok(ActionResult::ok("已等待"))
            }
            Action::AskUser { .. } | Action::Finish { .. } => Ok(loop_only()),
        }
    }

    /// Encodes the WDA base64 PNG as RGB, downscaled so its long edge is at most the configured size.
    async fn encode_screenshot(&self, base64_png: &str) -> Option<Vec<u8>> {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(base64_png)
            .ok()?;
        let image = image::load_from_memory(&bytes).ok()?.to_rgb8();
        let (width, height) = (image.width(), image.height());
        if width == 0 || height == 0 {
            return None;
        }
        let scale = (f64::from(self.screenshot_max_side) / f64::from(width.max(height))).min(1.0);
        let target_width = ((f64::from(width) * scale).round() as u32).max(1);
        let target_height = ((f64::from(height) * scale).round() as u32).max(1);
        let resized = if (target_width, target_height) == (width, height) {
            image
        } else {
            image::imageops::resize(
                &image,
                target_width,
                target_height,
                image::imageops::FilterType::Triangle,
            )
        };
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(resized)
            .write_to(&mut out, image::ImageFormat::Png)
            .ok()?;
        Some(out.into_inner())
    }
}

#[async_trait]
impl Device for WdaDevice {
    async fn observe(&self) -> Result<ScreenState, DeviceError> {
        let mut state = self.state.lock().await;
        let (width, height) = self.read_size().await?;
        let screenshot = self
            .encode_screenshot(&self.client.screenshot().await?)
            .await;
        let snapshot = WdaSource::parse_details(&self.client.source().await?, width, height)?;
        let bundle_id = self
            .client
            .active_app_info()
            .await?
            .get("bundleId")
            .and_then(json_content);
        state.width = width;
        state.height = height;
        state.latest = snapshot.clone();
        Ok(ScreenState {
            screenshot_png: screenshot,
            width_px: width,
            height_px: height,
            package_name: bundle_id,
            activity_name: None,
            nodes: snapshot.nodes(),
            secure: false,
            captured_at_millis: xiaoman_api::now_millis(),
        })
    }

    async fn perform(&self, action: Action) -> ActionResult {
        if action.is_terminal() {
            return loop_only();
        }
        let mut state = self.state.lock().await;
        let result = match self.execute(&mut state, &action).await {
            Ok(result) => result,
            Err(error) => ActionResult::failed(error.to_string()),
        };
        if result.ok && !matches!(action, Action::Wait { .. }) && self.settle_millis > 0 {
            (self.sleep)(self.settle_millis).await;
        }
        result
    }

    async fn installed_apps(&self) -> Vec<AppInfo> {
        self.apps.clone()
    }
}

fn loop_only() -> ActionResult {
    ActionResult::failed("此动作由操作循环处理")
}

fn unknown_node(id: i32) -> ActionResult {
    ActionResult::failed(format!("未知 nodeId {id}"))
}

fn lookup(state: &State, node_id: i32) -> Option<&crate::wda_source::Entry> {
    usize::try_from(node_id)
        .ok()
        .and_then(|index| state.latest.entries.get(index))
}

fn clamp_x(x: i32, width: i32) -> i32 {
    x.clamp(0, (width - 1).max(0))
}

fn clamp_y(y: i32, height: i32) -> i32 {
    y.clamp(0, (height - 1).max(0))
}

/// `JsonPrimitive.doubleOrNull`: numbers as-is, numeric strings parsed.
fn json_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

impl From<WdaError> for DeviceError {
    fn from(error: WdaError) -> Self {
        DeviceError(error.to_string())
    }
}

impl From<WdaSourceError> for DeviceError {
    fn from(error: WdaSourceError) -> Self {
        DeviceError(error.to_string())
    }
}
