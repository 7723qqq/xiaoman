//! `McpScreen.kt` in GPUI: the catalog grouped by category, the enabled switches, the key fields,
//! the test results, and the hand-added servers below.

use gpui::{
    ClickEvent, Context, Div, Entity, IntoElement, Render, ScrollHandle, Window, div, prelude::*,
};

use xiaoman_app::session::Session;
use xiaoman_mcp::McpPlatform;

use crate::theme::{Theme, theme};
use crate::view_model::copy;
use crate::view_model::mcp::{CustomRow, McpModel, ServerRow, StatusRow};

use super::components::{
    body, card, chip, column, divider, filled_button, heading, label, outlined_button, row, small,
    text_button, title, title_small, toggle,
};
use super::text_field::{
    Input, InputEvent, TextField, field_value, set_field_value, text_field, text_field_masked,
};

/// The MCP screen.
pub struct McpView {
    model: McpModel,
    session: Session,
    key_fields: Vec<(String, Entity<TextField>)>,
    custom_title: Entity<TextField>,
    custom_url: Entity<TextField>,
    custom_auth: Entity<TextField>,
    scroll: ScrollHandle,
}

impl McpView {
    /// The saved selection of `session` over the packaged catalog.
    pub fn new(window: &mut Window, cx: &mut Context<Self>, session: Session) -> Self {
        let model = McpModel::bundled(session.mcp_config(), McpPlatform::Desktop);
        let mut key_fields = Vec::new();
        for section in model.sections() {
            for server in section.servers {
                for key in server.keys {
                    let value = key.value.clone();
                    let name = key.name.clone();
                    let input = text_field_masked(window, cx, "", &value);
                    let bound = name.clone();
                    cx.subscribe_in(
                        &input,
                        window,
                        move |this, state, event: &InputEvent, _window, cx| {
                            if matches!(event, InputEvent::Change) {
                                this.model.set_key(&bound, &field_value(state, cx));
                                cx.notify();
                            }
                        },
                    )
                    .detach();
                    key_fields.push((name, input));
                }
            }
        }

        let title = model.custom_form().title_value;
        let url = model.custom_form().url_value;
        let auth = model.custom_form().auth_value;
        let custom_title = text_field(window, cx, copy::MCP_CUSTOM_NAME, &title);
        cx.subscribe_in(
            &custom_title,
            window,
            |this, state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.model.set_custom_title(field_value(state, cx));
                    cx.notify();
                }
            },
        )
        .detach();
        let custom_url = text_field(window, cx, copy::MCP_CUSTOM_URL, &url);
        cx.subscribe_in(
            &custom_url,
            window,
            |this, state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.model.set_custom_url(field_value(state, cx));
                    cx.notify();
                }
            },
        )
        .detach();
        let custom_auth = text_field_masked(window, cx, copy::MCP_CUSTOM_AUTH, &auth);
        cx.subscribe_in(
            &custom_auth,
            window,
            |this, state, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.model.set_custom_auth(field_value(state, cx));
                    cx.notify();
                }
            },
        )
        .detach();

        Self {
            model,
            session,
            key_fields,
            custom_title,
            custom_url,
            custom_auth,
            scroll: ScrollHandle::new(),
        }
    }

    pub fn model(&self) -> &McpModel {
        &self.model
    }

    /// Saves the ticked services and their keys through the session.
    fn save(&mut self, cx: &mut Context<Self>) {
        // `save` turns the note on; only a written file should say 已保存, so write first.
        let selection = self.model.selection().clone();
        match self.session.save_mcp(selection) {
            Ok(()) => {
                self.model.save();
            }
            Err(error) => eprintln!("保存外部服务失败：{error}"),
        }
        cx.notify();
    }

    /// Connects to the ticked services on a background thread and shows each result.
    fn test(&mut self, cx: &mut Context<Self>) {
        if self.model.testing() {
            return;
        }
        self.model.set_testing(true);
        let selection = self.model.selection().clone();
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let statuses = cx
                .background_executor()
                .spawn(async move { session.test_mcp(&selection) })
                .await;
            this.update(cx, |this, cx| {
                this.model.record_statuses(statuses);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn key_field(&self, name: &str) -> Option<&Entity<TextField>> {
        self.key_fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, input)| input)
    }

    fn status_line(theme: &Theme, status: &StatusRow) -> Div {
        let color = if status.ok() {
            theme.primary
        } else {
            theme.error
        };
        div()
            .text_size(Theme::small_size())
            .text_color(color)
            .child(status.label().to_string())
    }

    fn server_card(&self, theme: &Theme, server: &ServerRow, cx: &mut Context<Self>) -> Div {
        let id = server.id.clone();
        let on_toggle = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.model.toggle_enabled(&id);
            cx.notify();
        });

        let mut content = column().gap_2().p_3().child(
            row()
                .gap_2()
                .child(
                    column()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            row()
                                .gap_2()
                                .child(title_small(theme, server.title.clone()))
                                .child(label(theme, server.badge.clone(), theme.primary)),
                        )
                        .child(small(theme, server.description.clone())),
                )
                .child(toggle(
                    format!("server-toggle-{}", server.id),
                    theme,
                    server.enabled,
                    on_toggle,
                )),
        );

        if server.enabled {
            for key in &server.keys {
                if let Some(input) = self.key_field(&key.name) {
                    content = content
                        .child(
                            div()
                                .text_size(Theme::label_size())
                                .text_color(theme.on_surface_variant)
                                .child(key.label.clone()),
                        )
                        .child(Input::new(input).mask_toggle());
                }
                if let Some(help) = &key.help {
                    let url = help.clone();
                    let open = cx.listener(move |_this, _event: &ClickEvent, _window, cx| {
                        cx.open_url(&url);
                    });
                    content = content.child(text_button(
                        format!("apply-{}-{}", server.id, key.name),
                        theme,
                        copy::MCP_APPLY_KEY,
                        theme.primary,
                        true,
                        open,
                    ));
                }
            }
            if let Some(missing) = &server.missing {
                content = content.child(
                    div()
                        .text_size(Theme::small_size())
                        .text_color(theme.error)
                        .child(missing.clone()),
                );
            }
        }

        if let Some(status) = &server.status {
            content = content.child(Self::status_line(theme, status));
        }

        div().w_full().child(card(theme).child(content))
    }

    fn custom_card(
        &self,
        theme: &Theme,
        index: usize,
        custom: &CustomRow,
        cx: &mut Context<Self>,
    ) -> Div {
        let on_remove = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.model.remove_custom(index);
            cx.notify();
        });
        let mut content = column().gap_1().p_3().child(
            row()
                .child(
                    column()
                        .flex_1()
                        .min_w_0()
                        .child(title_small(theme, custom.title.clone()))
                        .child(small(theme, custom.detail.clone())),
                )
                .child(text_button(
                    format!("remove-{index}"),
                    theme,
                    copy::MCP_REMOVE,
                    theme.error,
                    true,
                    on_remove,
                )),
        );
        if let Some(status) = &custom.status {
            content = content.child(Self::status_line(theme, status));
        }
        div().w_full().child(card(theme).child(content))
    }

    fn custom_form(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let form = self.model.custom_form();
        let mut chips = row().gap_2();
        for (policy, label_text, selected) in form.confirm_options.iter().cloned() {
            let on_click = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                this.model.set_custom_confirm(policy);
                cx.notify();
            });
            chips = chips.child(chip(
                format!("mcp-confirm-{}", policy as u8),
                theme,
                label_text,
                selected,
                on_click,
            ));
        }

        let add = cx.listener(|this, _event: &ClickEvent, window, cx| {
            if this.model.add_custom() {
                set_field_value(&this.custom_title, "", window, cx);
                set_field_value(&this.custom_url, "", window, cx);
                set_field_value(&this.custom_auth, "", window, cx);
            }
            cx.notify();
        });

        column()
            .gap_2()
            .child(title(theme, copy::MCP_CUSTOM_TITLE))
            .child(small(theme, copy::MCP_CUSTOM_HINT))
            .child(
                div()
                    .text_size(Theme::label_size())
                    .text_color(theme.on_surface_variant)
                    .child(form.title_label),
            )
            .child(Input::new(&self.custom_title))
            .child(
                div()
                    .text_size(Theme::label_size())
                    .text_color(theme.on_surface_variant)
                    .child(form.url_label),
            )
            .child(Input::new(&self.custom_url))
            .children(form.problem.clone().map(|problem| {
                div()
                    .text_size(Theme::small_size())
                    .text_color(theme.error)
                    .child(problem)
            }))
            .child(
                div()
                    .text_size(Theme::label_size())
                    .text_color(theme.on_surface_variant)
                    .child(form.auth_label),
            )
            .child(self.custom_auth.clone())
            .child(
                div()
                    .text_size(Theme::body_size())
                    .text_color(theme.on_surface)
                    .child(form.confirm_title),
            )
            .child(chips)
            .child(
                div()
                    .when(!form.can_add, |element| element.opacity(0.6))
                    .child(filled_button(
                        "mcp-add",
                        theme,
                        form.add_label,
                        form.can_add,
                        add,
                    )),
            )
    }
}

impl Render for McpView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let count = self.model.enabled_count_label();
        let testing = self.model.testing();
        let saved = self.model.saved();

        let recommended = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.model.apply_recommended();
            cx.notify();
        });
        let save = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.save(cx);
        });
        let test = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.test(cx);
        });

        let mut body_column = column().gap_2();
        for section in self.model.sections() {
            body_column = body_column.child(
                div()
                    .pt_2()
                    .text_size(Theme::title_size())
                    .font_weight(Theme::SEMIBOLD)
                    .text_color(theme.on_surface)
                    .child(section.category),
            );
            for server in &section.servers {
                body_column = body_column.child(self.server_card(&theme, server, cx));
            }
        }

        let mut customs = column().gap_2();
        for (index, custom) in self.model.custom_rows().iter().enumerate() {
            customs = customs.child(self.custom_card(&theme, index, custom, cx));
        }

        let test_label = if testing {
            copy::MCP_TESTING
        } else {
            copy::MCP_TEST
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .child(
                div()
                    .id("mcp-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p_4()
                    .gap_2()
                    .child(heading(&theme, copy::MCP_TITLE))
                    .child(body(&theme, copy::MCP_INTRO).text_color(theme.on_surface))
                    .child(
                        row()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(Theme::body_size())
                                    .text_color(theme.on_surface)
                                    .child(count),
                            )
                            .child(outlined_button(
                                "mcp-recommended",
                                &theme,
                                copy::MCP_RECOMMENDED,
                                true,
                                recommended,
                            )),
                    )
                    .child(body_column)
                    .child(divider(&theme))
                    .child(customs)
                    .child(self.custom_form(&theme, cx))
                    .child(
                        row()
                            .gap_3()
                            .py_2()
                            .child(filled_button(
                                "mcp-save",
                                &theme,
                                copy::SETTINGS_SAVE,
                                true,
                                save,
                            ))
                            .child(outlined_button(
                                "mcp-test", &theme, test_label, !testing, test,
                            ))
                            .children(saved.then(|| small(&theme, copy::SETTINGS_SAVED))),
                    ),
            )
    }
}
