//! `SettingsScreen.kt` in GPUI: two endpoint sections, the agent knobs, Save, and the folders.

use gpui::{
    ClickEvent, Context, Div, Entity, EventEmitter, IntoElement, Render, ScrollHandle,
    SharedString, Window, div, prelude::*,
};

use xiaoman_app::session::Session;

use crate::theme::{Theme, theme};
use crate::view_model::copy;
use crate::view_model::settings::{Endpoint, Field, ModelRows, SettingsModel};

use super::components::{
    column, divider, field_block, field_error, filled_button, heading, row, small, text_button,
    title, toggle,
};
use super::text_field::{FieldChanged, TextField};

/// Told to the shell after a successful save, so the chat header can leave `待设置`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsSaved {
    pub configured: bool,
}

struct Binding {
    endpoint: Endpoint,
    field: Field,
    input: Entity<TextField>,
}

/// The settings screen.
pub struct SettingsView {
    model: SettingsModel,
    session: Session,
    bindings: Vec<Binding>,
    max_steps: Entity<TextField>,
    scroll: ScrollHandle,
}

impl EventEmitter<SettingsSaved> for SettingsView {}

impl SettingsView {
    /// The screen for `session`'s persisted settings; the folders are the session's own.
    pub fn new(cx: &mut Context<Self>, session: Session) -> Self {
        let model = SettingsModel::new(&session.settings()).with_folders(
            session.skills_dir().display().to_string(),
            session.memory_dir().display().to_string(),
            session.traces_dir().display().to_string(),
        );
        let mut bindings = Vec::new();
        for endpoint in [Endpoint::Chat, Endpoint::Operator] {
            for field in [
                Field::BaseUrl,
                Field::Model,
                Field::ApiKey,
                Field::Temperature,
            ] {
                let value = model.field(endpoint, field).to_string();
                let masked = field == Field::ApiKey && !model.revealed(endpoint);
                let input = cx.new(|cx| TextField::new(cx, "", &value).masked(masked));
                cx.subscribe(&input, move |this, _input, event: &FieldChanged, cx| {
                    this.model
                        .set_field(endpoint, field, event.text.to_string());
                    cx.notify();
                })
                .detach();
                bindings.push(Binding {
                    endpoint,
                    field,
                    input,
                });
            }
        }

        let max_steps = cx.new(|cx| TextField::new(cx, "", model.max_steps()));
        cx.subscribe(&max_steps, |this, _input, event: &FieldChanged, cx| {
            this.model.set_max_steps(event.text.to_string());
            cx.notify();
        })
        .detach();

        Self {
            model,
            session,
            bindings,
            max_steps,
            scroll: ScrollHandle::new(),
        }
    }

    pub fn model(&self) -> &SettingsModel {
        &self.model
    }

    fn binding(&self, endpoint: Endpoint, field: Field) -> &Entity<TextField> {
        &self
            .bindings
            .iter()
            .find(|binding| binding.endpoint == endpoint && binding.field == field)
            .expect("every field is bound at construction")
            .input
    }

    fn set_binding_text(
        &mut self,
        endpoint: Endpoint,
        field: Field,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let input = self.binding(endpoint, field).clone();
        input.update(cx, |input, cx| input.set_text(value, cx));
    }

    fn field_label(theme: &Theme, text: impl Into<SharedString>) -> Div {
        div()
            .text_size(Theme::label_size())
            .text_color(theme.on_surface_variant)
            .child(text.into())
    }

    fn key_button(
        &self,
        theme: &Theme,
        endpoint: Endpoint,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let revealed = self.model.revealed(endpoint);
        let label = if revealed {
            copy::SETTINGS_HIDE_KEY
        } else {
            copy::SETTINGS_SHOW_KEY
        };
        let on_click = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.model.toggle_reveal(endpoint);
            let masked = !this.model.revealed(endpoint);
            let input = this.binding(endpoint, Field::ApiKey).clone();
            input.update(cx, |input, cx| input.set_masked(masked, cx));
            cx.notify();
        });
        text_button(theme, label, theme.primary, true, on_click)
    }

    fn api_key_block(
        &self,
        theme: &Theme,
        rows: &ModelRows,
        endpoint: Endpoint,
        cx: &mut Context<Self>,
    ) -> Div {
        column()
            .gap_1()
            .py_1()
            .child(
                row()
                    .child(
                        div()
                            .flex_1()
                            .text_size(Theme::label_size())
                            .text_color(theme.on_surface_variant)
                            .child(rows.api_key.label.clone()),
                    )
                    .child(self.key_button(theme, endpoint, cx)),
            )
            .child(self.binding(endpoint, Field::ApiKey).clone())
    }

    fn model_section(
        &self,
        theme: &Theme,
        rows: &ModelRows,
        endpoint: Endpoint,
        cx: &mut Context<Self>,
    ) -> Div {
        column()
            .gap_2()
            .child(title(theme, rows.title.clone()))
            .child(field_block(
                theme,
                rows.base_url.label.clone(),
                self.binding(endpoint, Field::BaseUrl).clone(),
            ))
            .child(field_block(
                theme,
                rows.model.label.clone(),
                self.binding(endpoint, Field::Model).clone(),
            ))
            .child(self.api_key_block(theme, rows, endpoint, cx))
            .child(
                column()
                    .gap_1()
                    .py_1()
                    .child(Self::field_label(theme, rows.temperature.label.clone()))
                    .child(self.binding(endpoint, Field::Temperature).clone())
                    .children(
                        rows.temperature
                            .help
                            .clone()
                            .map(|help| field_error(theme, help)),
                    ),
            )
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let rows = self.model.rows();

        let copy_chat = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.model.copy_chat_to_operator();
            let base_url = this
                .model
                .field(Endpoint::Operator, Field::BaseUrl)
                .to_string();
            this.set_binding_text(Endpoint::Operator, Field::BaseUrl, base_url, cx);
            let key = this
                .model
                .field(Endpoint::Operator, Field::ApiKey)
                .to_string();
            this.set_binding_text(Endpoint::Operator, Field::ApiKey, key, cx);
            cx.notify();
        });

        let save = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            // Parse first: the "已保存" note only turns on when the files were actually written.
            let Some(settings) = this.model.to_settings() else {
                return;
            };
            match this.session.save_settings(settings) {
                Ok(()) => {
                    this.model.save();
                    cx.emit(SettingsSaved {
                        configured: this.session.configured(),
                    });
                }
                Err(error) => eprintln!("保存设置失败：{error}"),
            }
            cx.notify();
        });

        let learn = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.model.toggle_learn();
            cx.notify();
        });

        let saved = rows.saved_label.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .child(
                div()
                    .id("settings-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p_4()
                    .gap_2()
                    .child(heading(&theme, rows.title.clone()))
                    .child(self.model_section(&theme, &rows.chat, Endpoint::Chat, cx))
                    .child(divider(&theme))
                    .child(self.model_section(&theme, &rows.operator, Endpoint::Operator, cx))
                    .child(text_button(
                        &theme,
                        rows.copy_chat.clone(),
                        theme.primary,
                        true,
                        copy_chat,
                    ))
                    .child(divider(&theme))
                    .child(title(&theme, rows.agent_title.clone()))
                    .child(
                        column()
                            .gap_1()
                            .child(Self::field_label(&theme, rows.max_steps.label.clone()))
                            .child(self.max_steps.clone())
                            .children(
                                rows.max_steps
                                    .help
                                    .clone()
                                    .map(|help| field_error(&theme, help)),
                            ),
                    )
                    .child(
                        row()
                            .gap_3()
                            .py_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(Theme::body_size())
                                    .text_color(theme.on_surface)
                                    .child(rows.learn.label.clone()),
                            )
                            .child(toggle(&theme, rows.learn.on, learn)),
                    )
                    .child(
                        row()
                            .gap_3()
                            .py_2()
                            .child(filled_button(
                                &theme,
                                rows.save.label.clone(),
                                rows.save.enabled,
                                save,
                            ))
                            .children(saved.map(|label| small(&theme, label))),
                    )
                    .child(divider(&theme))
                    .child(title(&theme, rows.folders_title.clone()))
                    .child(
                        column().gap_1().children(
                            rows.folders
                                .iter()
                                .map(|folder| small(&theme, folder.clone())),
                        ),
                    ),
            )
    }
}

#[allow(dead_code)]
fn _keep_px(_: gpui::Pixels) {}
