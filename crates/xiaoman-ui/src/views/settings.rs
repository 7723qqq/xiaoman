//! `SettingsScreen.kt` in GPUI: two endpoint sections, the agent knobs, Save, and the folders.

use gpui::{
    ClickEvent, Context, Div, Entity, EventEmitter, IntoElement, Render, ScrollHandle,
    SharedString, Window, div, prelude::*, px,
};

use xiaoman_app::model_probe::{self, ProbeOutcome};
use xiaoman_app::session::Session;

use crate::theme::{Theme, theme};
use crate::view_model::copy;
use crate::view_model::settings::{Endpoint, Field, ModelRows, SettingsModel};

use super::components::{
    column, divider, field_block, field_error, filled_button, heading, outlined_button, row, small,
    text_button, title, toggle,
};
use super::text_field::{
    InputEvent, TextField, field_value, input, masked_input, set_field_value, text_field,
    text_field_masked,
};

/// Which probe the two buttons under an endpoint run.
#[derive(Debug, Clone, Copy)]
enum ProbeKind {
    Models,
    Test,
}

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
    pub fn new(window: &mut Window, cx: &mut Context<Self>, session: Session) -> Self {
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
                let input = if field == Field::ApiKey {
                    text_field_masked(window, cx, "", &value)
                } else {
                    text_field(window, cx, "", &value)
                };
                cx.subscribe_in(
                    &input,
                    window,
                    move |this, state, event: &InputEvent, _window, cx| {
                        if matches!(event, InputEvent::Change) {
                            this.model
                                .set_field(endpoint, field, field_value(state, cx));
                            cx.notify();
                        }
                    },
                )
                .detach();
                bindings.push(Binding {
                    endpoint,
                    field,
                    input,
                });
            }
        }

        let max_steps = text_field(window, cx, "", model.max_steps());
        cx.subscribe_in(
            &max_steps,
            window,
            |this, state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.model.set_max_steps(field_value(state, cx));
                    cx.notify();
                }
            },
        )
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = self.binding(endpoint, field).clone();
        set_field_value(&input, value, window, cx);
    }

    fn field_label(theme: &Theme, text: impl Into<SharedString>) -> Div {
        div()
            .text_size(Theme::label_size())
            .text_color(theme.on_surface_variant)
            .child(text.into())
    }

    /// The API key field: masked by the component, with its own eye toggle.
    fn api_key_block(&self, theme: &Theme, rows: &ModelRows, endpoint: Endpoint) -> Div {
        column()
            .gap_1()
            .py_1()
            .child(
                div()
                    .text_size(Theme::label_size())
                    .text_color(theme.on_surface_variant)
                    .child(rows.api_key.label.clone()),
            )
            .child(masked_input(self.binding(endpoint, Field::ApiKey)))
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
                input(self.binding(endpoint, Field::BaseUrl)),
            ))
            .child(self.model_row(theme, rows, endpoint, cx))
            .child(self.api_key_block(theme, rows, endpoint))
            .child(
                column()
                    .gap_1()
                    .py_1()
                    .child(Self::field_label(theme, rows.temperature.label.clone()))
                    .child(input(self.binding(endpoint, Field::Temperature)))
                    .children(
                        rows.temperature
                            .help
                            .clone()
                            .map(|help| field_error(theme, help)),
                    ),
            )
            .child(self.probe_block(theme, rows, endpoint, cx))
    }

    /// `模型名称` with the dropdown caret on its right: after `获取模型`, picking from the caret
    /// fills the field.
    fn model_row(
        &self,
        theme: &Theme,
        rows: &ModelRows,
        endpoint: Endpoint,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut field_row = row().gap_2().py_1().child(
            div()
                .flex_1()
                .min_w_0()
                .child(input(self.binding(endpoint, Field::Model))),
        );

        if !rows.models.is_empty() {
            let caret = if rows.picker_open { "▴" } else { "▾" };
            let toggle = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                this.model.toggle_picker(endpoint);
                cx.notify();
            });
            field_row = field_row.child(
                div()
                    .id(SharedString::from(format!(
                        "model-picker-{}",
                        endpoint_label(endpoint)
                    )))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(Theme::control_height())
                    .h(Theme::control_height())
                    .rounded_md()
                    .border_1()
                    .border_color(theme.outline_variant)
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(theme.on_surface_variant)
                    .hover(|style| style.bg(theme.surface_variant))
                    .child(caret)
                    .on_click(toggle),
            );
        }

        let mut block = column()
            .gap_1()
            .py_1()
            .child(Self::field_label(theme, rows.model.label.clone()))
            .child(field_row);

        if rows.picker_open {
            block = block.child(self.model_picker(theme, rows, endpoint, cx));
        }
        block
    }

    /// The list the caret opens: one row per model, capped in height and scrollable.
    fn model_picker(
        &self,
        theme: &Theme,
        rows: &ModelRows,
        endpoint: Endpoint,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut list = div()
            .id(SharedString::from(format!(
                "model-list-{}",
                endpoint_label(endpoint)
            )))
            .flex()
            .flex_col()
            .w_full()
            .p_1()
            .max_h(px(220.))
            .overflow_y_scroll()
            .rounded_lg()
            .border_1()
            .border_color(theme.outline_variant)
            .bg(theme.surface);

        for (index, model) in rows.models.iter().enumerate() {
            let label = model.clone();
            let picked = model.clone();
            let pick = cx.listener(move |this, _event: &ClickEvent, window, cx| {
                this.model.pick_model(endpoint, picked.clone());
                this.set_binding_text(endpoint, Field::Model, picked.clone(), window, cx);
                cx.notify();
            });
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "model-item-{}-{index}",
                        endpoint_label(endpoint)
                    )))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(Theme::small_size())
                    .text_color(theme.on_surface)
                    .hover(|style| style.bg(theme.surface_variant))
                    .child(label)
                    .on_click(pick),
            );
        }
        list
    }

    /// `获取模型` / `测试` and the result line of the last probe.
    fn probe_block(
        &self,
        theme: &Theme,
        rows: &ModelRows,
        endpoint: Endpoint,
        cx: &mut Context<Self>,
    ) -> Div {
        let fetch = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.probe(endpoint, ProbeKind::Models, cx);
        });
        let test = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.probe(endpoint, ProbeKind::Test, cx);
        });

        let mut block = column().gap_1().py_1().child(
            row()
                .gap_2()
                .child(outlined_button(
                    SharedString::from(format!("fetch-{}", endpoint_label(endpoint))),
                    theme,
                    copy::SETTINGS_FETCH_MODELS,
                    !rows.probing,
                    fetch,
                ))
                .child(outlined_button(
                    SharedString::from(format!("test-{}", endpoint_label(endpoint))),
                    theme,
                    copy::SETTINGS_TEST,
                    !rows.probing,
                    test,
                )),
        );

        if let Some(probe) = &rows.probe {
            let color = if probe.ok {
                theme.on_surface_variant
            } else {
                theme.error
            };
            block = block.child(
                div()
                    .text_size(Theme::small_size())
                    .text_color(color)
                    .child(probe.message.clone()),
            );
        }
        block
    }

    /// Runs one probe on a background thread against the draft being edited.
    fn probe(&mut self, endpoint: Endpoint, kind: ProbeKind, cx: &mut Context<Self>) {
        if self.model.probing(endpoint) {
            return;
        }
        let Some(settings) = self.model.draft(endpoint) else {
            self.model.apply_probe(
                endpoint,
                ProbeOutcome {
                    models: None,
                    message: copy::SETTINGS_INVALID_NUMBER.to_string(),
                    ok: false,
                },
            );
            cx.notify();
            return;
        };
        let session = self.session.clone();
        self.model.set_probing(endpoint, true);
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    match kind {
                        ProbeKind::Models => model_probe::models_outcome(
                            session.on_runtime(model_probe::fetch_models(&settings)),
                        ),
                        ProbeKind::Test => model_probe::test_outcome(
                            session.on_runtime(model_probe::test_model(&settings)),
                        ),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.model.apply_probe(endpoint, outcome);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let rows = self.model.rows();

        let copy_chat = cx.listener(|this, _event: &ClickEvent, window, cx| {
            this.model.copy_chat_to_operator();
            let base_url = this
                .model
                .field(Endpoint::Operator, Field::BaseUrl)
                .to_string();
            this.set_binding_text(Endpoint::Operator, Field::BaseUrl, base_url, window, cx);
            let key = this
                .model
                .field(Endpoint::Operator, Field::ApiKey)
                .to_string();
            this.set_binding_text(Endpoint::Operator, Field::ApiKey, key, window, cx);
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
                        "settings-copy-chat",
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
                            .child(input(&self.max_steps))
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
                            .child(toggle("learn-toggle", &theme, rows.learn.on, learn)),
                    )
                    .child(
                        row()
                            .gap_3()
                            .py_2()
                            .child(filled_button(
                                "settings-save",
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

/// Element ids must be unique across the two sections.
fn endpoint_label(endpoint: Endpoint) -> &'static str {
    match endpoint {
        Endpoint::Chat => "chat",
        Endpoint::Operator => "operator",
    }
}
