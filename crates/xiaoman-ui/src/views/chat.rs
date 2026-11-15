//! The chat screen: the header chip, the banners, the folded transcript, the approval card and
//! the skill draft card pinned above the input, and the input row itself.

use gpui::{
    App, ClickEvent, Context, Div, Entity, EventEmitter, IntoElement, Render, ScrollHandle,
    SharedString, Window, div, prelude::*, px,
};

use xiaoman_api::SkillAction;
use xiaoman_app::session::Session;

use crate::theme::{Theme, theme};
use crate::view_model::chat::{ChatModel, ChatRow, MessageKind, MessageRow, ProcessRow};
use crate::view_model::copy;

use super::components::{
    banner, body, column, filled_button, glyph_box, label, outlined_button, pill, row, text_button,
    title_small,
};
use super::text_field::{
    InputEvent, TextField, field_value, input, set_field_value, styled_text, text_field,
};

/// What the chat screen asks the shell to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    /// The "去设置" banner action.
    OpenSettings,
}

/// The chat screen.
pub struct ChatView {
    model: ChatModel,
    session: Session,
    draft: Entity<TextField>,
    scroll: ScrollHandle,
    scrolled_rows: usize,
}

impl EventEmitter<ChatEvent> for ChatView {}

impl ChatView {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        model: ChatModel,
        session: Session,
    ) -> Self {
        let placeholder = if model.question().is_some() {
            copy::CHAT_ANSWER_PLACEHOLDER
        } else {
            copy::CHAT_PLACEHOLDER
        };
        let draft = text_field(window, cx, placeholder, "");
        cx.subscribe_in(
            &draft,
            window,
            |this, _field, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.send(window, cx);
                }
            },
        )
        .detach();

        Self {
            model,
            session,
            draft,
            scroll: ScrollHandle::new(),
            scrolled_rows: 0,
        }
    }

    pub fn model(&self) -> &ChatModel {
        &self.model
    }

    pub fn set_configured(&mut self, configured: bool) {
        self.model.set_configured(configured);
    }

    pub fn draft(&self) -> &Entity<TextField> {
        &self.draft
    }

    /// Copies the session's transcript, status and pending prompts into the screen.
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.session.state();
        self.model.apply_snapshot(&snapshot);
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = field_value(&self.draft, cx);
        let outcome = self.session.send(&text);
        if outcome.clears_draft() {
            set_field_value(&self.draft, "", window, cx);
        }
        self.sync(cx);
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.session.stop();
        self.sync(cx);
    }

    fn fill_draft(&mut self, prompt: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        set_field_value(&self.draft, prompt, window, cx);
    }

    fn header(&self, theme: &Theme) -> Div {
        let header = self.model.header();
        row()
            .w_full()
            .px_5()
            .py_3()
            .gap_3()
            .child(glyph_box(
                theme,
                "满",
                px(38.),
                theme.primary,
                theme.primary_container,
            ))
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(Theme::title_size())
                            .font_weight(Theme::BOLD)
                            .text_color(theme.on_surface)
                            .child(header.title),
                    )
                    .child(label(theme, header.subtitle, theme.on_surface_variant)),
            )
            .child(pill(
                theme,
                header.status,
                theme.on_primary_container,
                theme.primary_container,
            ))
    }

    fn welcome(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let mut examples = column().gap_3();
        for (index, (example_title, example_detail, prompt)) in copy::EXAMPLES.iter().enumerate() {
            let prompt: &'static str = prompt;
            let on_click = cx.listener(move |this, _event: &ClickEvent, window, cx| {
                this.fill_draft(prompt, window, cx);
            });
            examples = examples.child(
                div()
                    .id(SharedString::from(format!("example-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .p_4()
                    .gap_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.outline_variant)
                    .bg(theme.surface)
                    .cursor_pointer()
                    .child(glyph_box(
                        theme,
                        "＋",
                        px(23.),
                        theme.primary,
                        theme.primary_container,
                    ))
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(Theme::body_size())
                                    .font_weight(Theme::MEDIUM)
                                    .text_color(theme.on_surface)
                                    .child(*example_title),
                            )
                            .child(label(theme, *example_detail, theme.on_surface_variant)),
                    )
                    .child(label(theme, "›", theme.on_surface_variant))
                    .on_click(on_click),
            );
        }

        column()
            .w_full()
            .pt_6()
            .gap_3()
            .child(
                div()
                    .text_size(Theme::headline_size())
                    .font_weight(Theme::SEMIBOLD)
                    .text_color(theme.on_surface)
                    .child(copy::CHAT_WELCOME_TITLE),
            )
            .child(body(theme, copy::CHAT_WELCOME_BODY).text_color(theme.on_surface_variant))
            .child(examples)
            .child(
                div()
                    .pt_2()
                    .text_size(Theme::small_size())
                    .text_color(theme.on_surface_variant)
                    .child(copy::CHAT_WELCOME_FOOTER),
            )
    }

    fn transcript(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        if self.model.items().is_empty() {
            return self.welcome(theme, cx);
        }
        let mut list = column().gap_4();
        for chat_row in self.model.rows() {
            list = match chat_row {
                ChatRow::Message(message) => list.child(message_bubble(theme, &message)),
                ChatRow::Process(card) => {
                    let id = card.id;
                    let on_click = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                        this.model.toggle(id);
                        cx.notify();
                    });
                    list.child(process_card(theme, &card, on_click))
                }
            };
        }
        list
    }

    fn status_line(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let status = self.model.status_line()?;
        let on_stop = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.stop(cx);
        });
        Some(
            row()
                .px_5()
                .py_1()
                .gap_2()
                .child(glyph_box(
                    theme,
                    "·",
                    px(14.),
                    theme.primary,
                    theme.primary_container,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(Theme::small_size())
                        .text_color(theme.on_surface)
                        .child(status.to_string()),
                )
                .child(text_button(
                    "chat-stop",
                    theme,
                    copy::CHAT_STOP,
                    theme.primary,
                    true,
                    on_stop,
                )),
        )
    }

    fn approval_card(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let Some(state) = self.model.approval() else {
            return div();
        };
        let responding = state.responding;
        let approval_id = state.id;

        let deny = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.session.deny(approval_id);
            this.sync(cx);
        });
        let allow_once = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.model.set_responding(true);
            this.session
                .approve(approval_id, xiaoman_api::ApprovalScope::Once);
            this.sync(cx);
        });
        let more = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.model.set_more(true);
            cx.notify();
        });
        let allow_task = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.model.set_responding(true);
            this.session
                .approve(approval_id, xiaoman_api::ApprovalScope::Task);
            this.sync(cx);
        });

        let mut controls = row()
            .gap_3()
            .child(outlined_button(
                "approval-deny",
                theme,
                copy::APPROVAL_DENY,
                !responding,
                deny,
            ))
            .child(filled_button(
                "approval-once",
                theme,
                copy::APPROVAL_ONCE,
                !responding,
                allow_once,
            ));

        controls = if state.more {
            controls.child(text_button(
                "approval-task",
                theme,
                copy::APPROVAL_TASK,
                theme.primary,
                !responding,
                allow_task,
            ))
        } else {
            controls.child(text_button(
                "approval-more",
                theme,
                copy::APPROVAL_MORE,
                theme.primary,
                !responding,
                more,
            ))
        };

        div().w_full().p_3().child(
            column()
                .w_full()
                .p_4()
                .gap_3()
                .rounded_xl()
                .bg(theme.secondary_container)
                .child(
                    title_small(theme, state.title.clone())
                        .text_color(theme.on_secondary_container),
                )
                .child(
                    div()
                        .id("approval-prompt")
                        .w_full()
                        .max_h(px(110.))
                        .overflow_y_scroll()
                        .text_size(Theme::body_size())
                        .line_height(Theme::body_line_height())
                        .text_color(theme.on_secondary_container)
                        .child(state.prompt.clone()),
                )
                .child(controls),
        )
    }

    fn skill_draft_card(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let Some(state) = self.model.skill_draft() else {
            return div();
        };
        let id = state.id;
        let save = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.session.answer_skill_draft(id, true);
            this.sync(cx);
        });
        let cancel = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.session.answer_skill_draft(id, false);
            this.sync(cx);
        });
        let action = match state.action {
            SkillAction::Create => copy::SKILL_DRAFT_CREATE,
            SkillAction::Update => copy::SKILL_DRAFT_UPDATE,
            SkillAction::Delete => copy::SKILL_DRAFT_DELETE,
        };
        let body = if state.content.is_empty() {
            state.previous.clone().unwrap_or_default()
        } else {
            state.content.clone()
        };

        let mut card = column()
            .w_full()
            .p_4()
            .gap_3()
            .rounded_xl()
            .bg(theme.secondary_container)
            .child(
                title_small(theme, format!("{} · {}", copy::SKILL_DRAFT_TITLE, action))
                    .text_color(theme.on_secondary_container),
            )
            .child(
                div()
                    .text_size(Theme::small_size())
                    .text_color(theme.on_secondary_container)
                    .child(state.name.clone()),
            )
            .child(
                div()
                    .id("skill-draft-preview")
                    .w_full()
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .text_size(Theme::small_size())
                    .line_height(Theme::body_line_height())
                    .text_color(theme.on_secondary_container)
                    .child(body),
            );
        if state.shadows_bundled {
            card = card.child(
                div()
                    .text_size(Theme::small_size())
                    .text_color(theme.primary)
                    .child(copy::SKILL_DRAFT_SHADOW),
            );
        }
        if state.action == SkillAction::Delete {
            card = card.child(
                div()
                    .text_size(Theme::small_size())
                    .text_color(theme.primary)
                    .child(copy::SKILL_DRAFT_DELETE_WARNING),
            );
        }

        div().w_full().p_3().child(
            card.child(
                row()
                    .gap_3()
                    .child(outlined_button(
                        "skill-cancel",
                        theme,
                        copy::SKILL_CANCEL,
                        true,
                        cancel,
                    ))
                    .child(filled_button(
                        "skill-save",
                        theme,
                        copy::SKILL_SAVE,
                        true,
                        save,
                    )),
            ),
        )
    }

    fn input_row(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let draft = field_value(&self.draft, cx);
        let can_send = self.model.can_send(&draft);
        let stop = self.model.shows_stop();

        let action = cx.listener(|this, _event: &ClickEvent, window, cx| {
            if this.model.shows_stop() {
                this.stop(cx);
            } else {
                this.send(window, cx);
            }
            cx.notify();
        });
        let enabled = stop || can_send;

        row()
            .w_full()
            .px_4()
            .py_2()
            .gap_2()
            .items_end()
            .child(div().flex_1().min_w_0().child(input(&self.draft)))
            .child(
                div()
                    .id("chat-send")
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .w(Theme::control_height())
                    .h(Theme::control_height())
                    .rounded_full()
                    .bg(if enabled {
                        theme.primary
                    } else {
                        theme.surface_variant
                    })
                    .text_size(px(20.))
                    .text_color(if enabled {
                        theme.on_primary
                    } else {
                        theme.on_surface_variant
                    })
                    .when(enabled, |element| element.cursor_pointer())
                    .child(if stop { "■" } else { "↑" })
                    .when(enabled, |element| element.on_click(action)),
            )
    }
}

impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let placeholder = if self.model.question().is_some() {
            copy::CHAT_ANSWER_PLACEHOLDER
        } else {
            copy::CHAT_PLACEHOLDER
        };
        self.draft.update(cx, |field, cx| {
            field.set_placeholder(SharedString::from(placeholder), window, cx)
        });

        let mut banners = column();
        for (index, item) in self.model.banners().into_iter().enumerate() {
            let open_settings = cx.listener(|_this, _event: &ClickEvent, _window, cx| {
                cx.emit(ChatEvent::OpenSettings);
            });
            banners = banners.child(banner(
                format!("banner-{index}"),
                &theme,
                item.text,
                item.action,
                open_settings,
            ));
        }

        let rows = self.model.rows().len();
        if rows != self.scrolled_rows {
            self.scrolled_rows = rows;
            self.scroll.scroll_to_bottom();
        }

        let bottom = if self.model.approval().is_some() {
            self.approval_card(&theme, cx)
        } else if self.model.skill_draft().is_some() {
            self.skill_draft_card(&theme, cx)
        } else {
            let mut area = column().w_full();
            if let Some(status) = self.status_line(&theme, cx) {
                area = area.child(status);
            }
            area.child(self.input_row(&theme, cx))
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .child(self.header(&theme))
            .child(banners)
            .child(
                div()
                    .id("transcript")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(px(18.))
                    .py(px(18.))
                    .child(self.transcript(&theme, cx)),
            )
            .child(bottom)
    }
}

fn rich_spans(spans: &[xiaoman_app::chat_item::TextSpan]) -> Vec<(std::ops::Range<usize>, bool)> {
    spans
        .iter()
        .map(|span| (span.start..span.end, true))
        .collect()
}

fn message_bubble(theme: &Theme, message: &MessageRow) -> Div {
    let rich = &message.body;

    match message.kind {
        MessageKind::User => row().justify_end().child(
            div()
                .max_w(px(560.))
                .px_4()
                .py_3()
                .rounded_xl()
                .bg(theme.primary_container)
                .text_size(Theme::body_size())
                .line_height(Theme::body_line_height())
                .text_color(theme.on_primary_container)
                .child(styled_text(&rich.text, theme, &rich_spans(&rich.spans))),
        ),
        MessageKind::Assistant => column()
            .gap_2()
            .child(
                div()
                    .text_size(Theme::label_size())
                    .font_weight(Theme::SEMIBOLD)
                    .text_color(theme.primary)
                    .child(copy::CHAT_ASSISTANT_NAME),
            )
            .child(
                div()
                    .w_full()
                    .p_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(theme.outline_variant)
                    .bg(theme.surface)
                    .child(styled_text(&rich.text, theme, &rich_spans(&rich.spans))),
            ),
        MessageKind::Question { .. } | MessageKind::Error => {
            let error = matches!(message.kind, MessageKind::Error);
            let (background, foreground) = if error {
                (theme.error_container, theme.on_error_container)
            } else {
                (theme.secondary_container, theme.on_secondary_container)
            };
            div().w_full().child(
                column()
                    .w_full()
                    .p_4()
                    .gap_2()
                    .rounded_xl()
                    .bg(background)
                    .child(
                        title_small(theme, message.title.clone().unwrap_or_default())
                            .text_color(foreground),
                    )
                    .child(
                        div()
                            .text_size(Theme::body_size())
                            .line_height(Theme::body_line_height())
                            .text_color(foreground)
                            .child(styled_text(&rich.text, theme, &rich_spans(&rich.spans))),
                    ),
            )
        }
    }
}

fn process_card(
    theme: &Theme,
    state: &ProcessRow,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let chevron = if state.expanded { "▾" } else { "▸" };
    let header = row()
        .id(SharedString::from(format!("process-{}", state.id)))
        .w_full()
        .p_3()
        .gap_2()
        .cursor_pointer()
        .child(label(theme, "≡", theme.on_surface_variant))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(Theme::small_size())
                .text_color(theme.on_surface)
                .child(state.header.clone()),
        )
        .child(label(theme, chevron, theme.on_surface_variant))
        .on_click(on_toggle);

    let mut card_body = column();
    if state.expanded {
        let mut entries = column().gap_2().px(px(14.)).pb(px(14.));
        for entry in &state.entries {
            let color = if entry.failed() {
                theme.error
            } else {
                theme.on_surface_variant
            };
            entries = entries.child(
                div()
                    .text_size(Theme::small_size())
                    .line_height(Theme::small_line_height())
                    .text_color(color)
                    .child(entry.text.clone()),
            );
        }
        card_body = card_body.child(entries);
    }

    div().w_full().child(
        div()
            .flex()
            .flex_col()
            .w_full()
            .rounded_xl()
            .border_1()
            .border_color(theme.outline_variant)
            .bg(theme.surface_container_low)
            .child(header)
            .child(card_body),
    )
}
