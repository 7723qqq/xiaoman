//! Settings: the two model endpoints, the step cap and the learning toggle.
//!
//! Same fields and defaults as the Compose `SettingsScreen` and the desktop
//! `view_model::settings`; saving writes through `Session::save_settings`.

use std::sync::Arc;

use gpui::{
    div, prelude::*, rgb, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::InputState;
use gpui_kit::component::switch::Switch;
use xiaoman_app::session::Session;
use xiaoman_app::settings::{AppSettings, ModelSettings};

use crate::ui::{field, hint, input, title, value};

pub struct SettingsView {
    session: Option<Arc<Session>>,
    chat_base: Entity<InputState>,
    chat_model: Entity<InputState>,
    chat_key: Entity<InputState>,
    chat_temperature: Entity<InputState>,
    operator_base: Entity<InputState>,
    operator_model: Entity<InputState>,
    operator_key: Entity<InputState>,
    operator_temperature: Entity<InputState>,
    max_steps: Entity<InputState>,
    learn: bool,
    message: Option<String>,
}

impl SettingsView {
    pub fn new(session: Option<Arc<Session>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = session
            .as_ref()
            .map(|session| session.settings())
            .unwrap_or_default();
        let chat_base = input(window, cx, "Base URL", settings.chat.base_url.as_str());
        let chat_model = input(window, cx, "模型名称", settings.chat.model.as_str());
        let chat_key = input(window, cx, "API Key", settings.chat.api_key.as_str());
        let chat_temperature = input(
            window,
            cx,
            "Temperature（可选）",
            temperature_text(settings.chat.temperature),
        );
        let operator_base = input(window, cx, "Base URL", settings.operator.base_url.as_str());
        let operator_model = input(window, cx, "模型名称", settings.operator.model.as_str());
        let operator_key = input(window, cx, "API Key", settings.operator.api_key.as_str());
        let operator_temperature = input(
            window,
            cx,
            "Temperature（可选）",
            temperature_text(settings.operator.temperature),
        );
        let max_steps = input(window, cx, "1-200", settings.max_steps.to_string());
        Self {
            session,
            chat_base,
            chat_model,
            chat_key,
            chat_temperature,
            operator_base,
            operator_model,
            operator_key,
            operator_temperature,
            max_steps,
            learn: settings.learn_from_success,
            message: None,
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            self.message = Some("会话还没有打开。".to_string());
            cx.notify();
            return;
        };
        let steps = value(&self.max_steps, cx);
        if !valid_steps(&steps)
            || !valid_temperature(&value(&self.chat_temperature, cx))
            || !valid_temperature(&value(&self.operator_temperature, cx))
        {
            self.message = Some("请输入有效的数字".to_string());
            cx.notify();
            return;
        }
        let settings = AppSettings {
            chat: self.model(
                &self.chat_base,
                &self.chat_model,
                &self.chat_key,
                &self.chat_temperature,
                cx,
            ),
            operator: self.model(
                &self.operator_base,
                &self.operator_model,
                &self.operator_key,
                &self.operator_temperature,
                cx,
            ),
            max_steps: steps.parse::<i32>().unwrap_or(30),
            learn_from_success: self.learn,
        };
        self.message = Some(match session.save_settings(settings) {
            Ok(()) => "已保存".to_string(),
            Err(error) => format!("保存失败：{error}"),
        });
        cx.notify();
    }

    fn model(
        &self,
        base: &Entity<InputState>,
        model: &Entity<InputState>,
        key: &Entity<InputState>,
        temperature: &Entity<InputState>,
        cx: &Context<Self>,
    ) -> ModelSettings {
        ModelSettings {
            base_url: value(base, cx),
            model: value(model, cx),
            api_key: value(key, cx),
            temperature: value(temperature, cx)
                .parse::<f64>()
                .ok()
                .filter(|value| (0.0..=2.0).contains(value)),
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (skills_dir, memory_dir, traces_dir) = self
            .session
            .as_ref()
            .map(|session| {
                (
                    session.skills_dir().display().to_string(),
                    session.memory_dir().display().to_string(),
                    session.traces_dir().display().to_string(),
                )
            })
            .unwrap_or_default();
        div().size_full().id("settings").overflow_y_scroll().child(
            div()
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(title("设置"))
                .child(hint(
                    "两个模型都填 OpenAI 兼容接口；操作模型需要能看图。Key 只保存在本机。",
                ))
                .child(title("聊天模型"))
                .child(field("Base URL", &self.chat_base))
                .child(field("模型名称", &self.chat_model))
                .child(field("API Key", &self.chat_key))
                .child(field("Temperature（可选）", &self.chat_temperature))
                .when(
                    !valid_temperature(&value(&self.chat_temperature, cx)),
                    |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(rgb(0xb3261e))
                                .child("请输入有效的数字"),
                        )
                    },
                )
                .child(title("操作模型（需支持图片）"))
                .child(field("Base URL", &self.operator_base))
                .child(field("模型名称", &self.operator_model))
                .child(field("API Key", &self.operator_key))
                .child(field("Temperature（可选）", &self.operator_temperature))
                .when(
                    !valid_temperature(&value(&self.operator_temperature, cx)),
                    |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(rgb(0xb3261e))
                                .child("请输入有效的数字"),
                        )
                    },
                )
                .child(
                    Button::new("copy-chat")
                        .label("与聊天模型相同")
                        .on_click(cx.listener(|this, _, window, cx| {
                            let base = value(&this.chat_base, cx);
                            let key = value(&this.chat_key, cx);
                            this.operator_base
                                .update(cx, |state, cx| state.set_value(base, window, cx));
                            this.operator_key
                                .update(cx, |state, cx| state.set_value(key, window, cx));
                            cx.notify();
                        })),
                )
                .child(title("任务"))
                .child(field("最多步数", &self.max_steps))
                .when(!valid_steps(&value(&self.max_steps, cx)), |this| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xb3261e))
                            .child("请输入有效的数字"),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_sm().child("成功后记住操作路径"))
                                .child(hint("任务成功时把路径写进记忆，下次执行时参考。")),
                        )
                        .child(
                            Switch::new("learn")
                                .checked(self.learn)
                                .on_change(cx.listener(|this, checked, _, cx| {
                                    this.learn = *checked;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    Button::new("save-settings")
                        .primary()
                        .label("保存")
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
                .when_some(self.message.clone(), |this, message| {
                    this.child(div().text_sm().child(message))
                })
                .child(title("宿主"))
                .child(status_row("无障碍服务", host_accessibility()))
                .child(status_row("前台服务", host_service()))
                .child(status_row("Rust 库", host_library()))
                .child(status_row(
                    "通知权限",
                    status_text(host_notifications(), "已允许", "未允许"),
                ))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("open-a11y")
                                .label("去开启无障碍服务")
                                .on_click(cx.listener(|_, _, _, _| open_accessibility_settings())),
                        )
                        .child(
                            Button::new("start-service")
                                .label("启动前台服务")
                                .on_click(cx.listener(|_, _, _, _| start_service())),
                        ),
                )
                .when(!host_notifications(), |this| {
                    this.child(
                        Button::new("open-notification-settings")
                            .label("去允许通知")
                            .on_click(cx.listener(|_, _, _, _| open_notification_settings())),
                    )
                })
                .child(title("文件夹"))
                .child(hint(format!("技能：{skills_dir}")))
                .child(hint(format!("记忆：{memory_dir}")))
                .child(hint(format!("轨迹：{traces_dir}")))
                .child(hint("无障碍服务由系统的「设置 → 无障碍 → 小满」开启。")),
        )
    }
}

fn temperature_text(temperature: Option<f64>) -> String {
    temperature
        .map(xiaoman_app::settings::double_text)
        .unwrap_or_default()
}

/// Empty or a number in 0..=2: the same rule the Compose form uses.
fn valid_temperature(text: &str) -> bool {
    let text = text.trim();
    text.is_empty()
        || text
            .parse::<f64>()
            .map(|value| (0.0..=2.0).contains(&value))
            .unwrap_or(false)
}

/// A whole number in 1..=200.
fn valid_steps(text: &str) -> bool {
    text.trim()
        .parse::<i32>()
        .map(|value| (1..=200).contains(&value))
        .unwrap_or(false)
}

fn status_row(label: &str, value: impl gpui::IntoElement) -> impl gpui::IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(div().text_sm().child(label.to_string()))
        .child(value)
}

fn status_text(on: bool, on_text: &'static str, off_text: &'static str) -> impl gpui::IntoElement {
    div()
        .text_sm()
        .text_color(if on {
            gpui::rgb(0x1e6b52)
        } else {
            gpui::rgb(0xb3261e)
        })
        .child(if on { on_text } else { off_text })
}

#[cfg(target_os = "android")]
fn host_accessibility() -> impl gpui::IntoElement {
    status_text(
        xiaoman_android::session::accessibility_enabled(),
        "已开启",
        "未开启",
    )
}
#[cfg(not(target_os = "android"))]
fn host_accessibility() -> impl gpui::IntoElement {
    status_text(false, "已开启", "未开启")
}

#[cfg(target_os = "android")]
fn host_service() -> impl gpui::IntoElement {
    status_text(
        xiaoman_android::session::service_running(),
        "运行中",
        "未运行",
    )
}
#[cfg(not(target_os = "android"))]
fn host_service() -> impl gpui::IntoElement {
    status_text(false, "运行中", "未运行")
}

#[cfg(target_os = "android")]
fn host_library() -> impl gpui::IntoElement {
    status_text(
        xiaoman_android::session::library_loaded(),
        "已加载",
        "未加载",
    )
}
#[cfg(not(target_os = "android"))]
fn host_library() -> impl gpui::IntoElement {
    status_text(false, "已加载", "未加载")
}

#[cfg(target_os = "android")]
fn host_notifications() -> bool {
    xiaoman_android::session::notifications_allowed()
}
#[cfg(not(target_os = "android"))]
fn host_notifications() -> bool {
    false
}

#[cfg(target_os = "android")]
fn start_service() {
    xiaoman_android::session::start_service();
}
#[cfg(not(target_os = "android"))]
fn start_service() {}

#[cfg(target_os = "android")]
fn open_accessibility_settings() {
    xiaoman_android::session::open_accessibility_settings();
}
#[cfg(not(target_os = "android"))]
fn open_accessibility_settings() {}

#[cfg(target_os = "android")]
fn open_notification_settings() {
    xiaoman_android::session::open_notification_settings();
}
#[cfg(not(target_os = "android"))]
fn open_notification_settings() {}

#[cfg(test)]
mod tests {
    use super::{temperature_text, valid_steps, valid_temperature};

    #[test]
    fn temperature_text_uses_kotlins_decimal_point() {
        assert_eq!("", temperature_text(None));
        assert_eq!("1.0", temperature_text(Some(1.0)));
        assert_eq!("0.7", temperature_text(Some(0.7)));
        assert_eq!("0.25", temperature_text(Some(0.25)));
    }

    #[test]
    fn temperature_and_steps_accept_the_same_ranges_as_the_form() {
        for ok in ["", "0", "0.0", "1", "2", "2.0", " 1.5 "] {
            assert!(valid_temperature(ok), "{ok}");
        }
        for bad in ["-0.1", "2.1", "abc", "NaN", "inf"] {
            assert!(!valid_temperature(bad), "{bad}");
        }
        for ok in ["1", "200", " 3 "] {
            assert!(valid_steps(ok), "{ok}");
        }
        for bad in ["0", "201", "-1", "1.5", ""] {
            assert!(!valid_steps(bad), "{bad}");
        }
    }
}
