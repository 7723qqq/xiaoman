//! `MemoryScreen.kt` in GPUI: the search box, the note list, one open note with its delete
//! confirmation, and the empty states.
//!
//! The list, the note and the delete all run against the session's `FileMemoryStore`; every
//! refresh replaces the rows with what is on disk now, so a note the agent wrote appears without a
//! restart.

use std::ops::Range;

use gpui::{ClickEvent, Context, Div, Entity, IntoElement, Render, Window, div, prelude::*};

use xiaoman_app::session::Session;

use crate::theme::{Theme, theme};
use crate::view_model::copy;
use crate::view_model::memory::{MemoryModel, MemoryRow};

use super::components::{column, heading, row, small, text_button};
use super::text_field::{FieldChanged, TextField, styled_text};

/// The memory screen.
pub struct MemoryView {
    model: MemoryModel,
    session: Session,
    search: Entity<TextField>,
}

impl MemoryView {
    pub fn new(cx: &mut Context<Self>, session: Session) -> Self {
        let model = MemoryModel::new();
        let search = cx.new(|cx| TextField::new(cx, copy::MEMORY_SEARCH_HINT, ""));
        cx.subscribe(&search, |this, _field, event: &FieldChanged, cx| {
            this.model.set_query(event.text.to_string());
            this.refresh(cx);
        })
        .detach();

        let mut view = Self {
            model,
            session,
            search,
        };
        view.reload(cx);
        view
    }

    pub fn model(&self) -> &MemoryModel {
        &self.model
    }

    /// `memory.list()` for a blank query, `memory.search(q, 50)` otherwise.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let query = self.model.query().trim().to_string();
        if query.is_empty() {
            match self.session.memory_list(None) {
                Ok(notes) => self.model.set_notes(&notes),
                Err(error) => {
                    eprintln!("读取记忆失败：{error}");
                    self.model.set_failed();
                }
            }
        } else {
            match self.session.memory_search(&query, 50) {
                Ok(hits) => self.model.set_hits(&hits),
                Err(error) => {
                    eprintln!("搜索记忆失败：{error}");
                    self.model.set_failed();
                }
            }
        }
        cx.notify();
    }

    /// The query changed: the rows follow it.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
    }

    /// Tapping a row: a note that is gone shows `MEMORY_NOT_FOUND` instead of an empty page.
    fn open(&mut self, permalink: &str, cx: &mut Context<Self>) {
        match self.session.memory_read(permalink) {
            Ok(Some(note)) => self.model.open_note(&note),
            Ok(None) => self.model.open_missing(),
            Err(error) => {
                eprintln!("打开记忆失败：{error}");
                self.model.open_missing();
            }
        }
        cx.notify();
    }

    /// The confirmation's `删除`: the file is deleted, the note closes, the list reloads.
    fn delete_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(permalink) = self.model.confirm_delete().map(str::to_string) else {
            return;
        };
        let message = match self.session.memory_delete(&permalink) {
            Ok(true) => "已删除。".to_string(),
            Ok(false) => "删除失败：文件不存在。".to_string(),
            Err(error) => format!("删除失败：{error}"),
        };
        self.model.deleted();
        self.model.set_message(Some(message));
        self.reload(cx);
    }

    fn row_card(&self, theme: &Theme, row: &MemoryRow, cx: &mut Context<Self>) -> impl IntoElement {
        let permalink = row.permalink.clone();
        let click = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.open(&permalink, cx);
        });
        div()
            .id(gpui::SharedString::from(format!(
                "memory-{}",
                row.permalink
            )))
            .flex()
            .flex_col()
            .w_full()
            .p_3()
            .gap_1()
            .rounded_lg()
            .bg(theme.surface)
            .border_1()
            .border_color(theme.outline_variant)
            .cursor_pointer()
            .hover(|style| style.bg(theme.surface_variant))
            .child(
                div()
                    .text_size(Theme::body_size())
                    .text_color(theme.on_surface)
                    .child(row.title.clone()),
            )
            .child(small(theme, row.detail.clone()))
            .on_click(click)
    }

    /// The open note: back on the left, the title, and the delete button on the right.
    fn note_page(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let Some(note) = self.model.open().cloned() else {
            return column();
        };

        let back = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.model.close();
            cx.notify();
        });
        let permalink = note.permalink.clone();
        let ask_delete = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.model.begin_delete(permalink.clone());
            cx.notify();
        });

        let mut page = column().gap_2().p_3().child(
            row()
                .gap_2()
                .child(text_button(
                    "memory-back",
                    theme,
                    copy::BACK,
                    theme.primary,
                    true,
                    back,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(heading(theme, note.title.clone())),
                )
                .child(text_button(
                    "memory-delete",
                    theme,
                    copy::MEMORY_DELETE,
                    theme.error,
                    true,
                    ask_delete,
                )),
        );

        if let Some(open_missing) = self.model.state().not_found() {
            page = page.child(
                div()
                    .text_size(Theme::small_size())
                    .text_color(theme.error)
                    .child(open_missing.to_string()),
            );
        } else {
            page = page.child(
                div()
                    .text_size(Theme::body_size())
                    .line_height(Theme::body_line_height())
                    .text_color(theme.on_surface)
                    .child(styled_text(
                        &note.body.text,
                        theme,
                        &rich_spans(&note.body.spans),
                    )),
            );
        }

        if self.model.confirm_delete().is_some() {
            page = page.child(self.delete_dialog(theme, &note.title, cx));
        }
        page
    }

    /// The Kotlin `AlertDialog`: title, the note's name, then `取消` / `删除`.
    fn delete_dialog(&self, theme: &Theme, title: &str, cx: &mut Context<Self>) -> Div {
        let confirm = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.delete_confirmed(cx);
        });
        let cancel = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.model.cancel_delete();
            cx.notify();
        });
        div()
            .flex()
            .flex_col()
            .w_full()
            .p_4()
            .gap_2()
            .rounded_lg()
            .bg(theme.surface)
            .border_1()
            .border_color(theme.outline_variant)
            .child(
                div()
                    .text_size(Theme::title_small_size())
                    .font_weight(Theme::SEMIBOLD)
                    .text_color(theme.on_surface)
                    .child(copy::MEMORY_DELETE_TITLE),
            )
            .child(small(theme, copy::memory_delete_message(title)))
            .child(
                row()
                    .gap_2()
                    .justify_end()
                    .child(text_button(
                        "memory-delete-cancel",
                        theme,
                        copy::CANCEL,
                        theme.primary,
                        true,
                        cancel,
                    ))
                    .child(text_button(
                        "memory-delete-confirm",
                        theme,
                        copy::MEMORY_DELETE_CONFIRM,
                        theme.error,
                        true,
                        confirm,
                    )),
            )
    }
}

impl Render for MemoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);

        let mut page = if self.model.open().is_some() {
            self.note_page(&theme, cx)
        } else {
            self.list_page(&theme, cx)
        };

        if let Some(message) = self.model.message().map(str::to_string) {
            let clear = cx.listener(|this, _event: &ClickEvent, _window, cx| {
                this.model.set_message(None);
                cx.notify();
            });
            page = page.child(
                row()
                    .gap_2()
                    .py_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(Theme::small_size())
                            .text_color(theme.on_surface_variant)
                            .child(message),
                    )
                    .child(text_button(
                        "memory-dismiss-message",
                        &theme,
                        copy::CLEAR,
                        theme.primary,
                        true,
                        clear,
                    )),
            );
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .child(
                div()
                    .id("memory-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(page),
            )
    }
}

impl MemoryView {
    /// The list: heading, search box with `清除`, the empty states and one card per row.
    fn list_page(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let clear = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.model.set_query("");
            this.search.update(cx, |input, cx| input.clear(cx));
            this.reload(cx);
        });

        let rows = self.model.rows().to_vec();
        let hint = self.model.state().empty_hint();

        let mut list = column().gap_2();
        for row in &rows {
            list = list.child(self.row_card(theme, row, cx));
        }

        let mut content = column()
            .gap_2()
            .p_3()
            .child(heading(theme, copy::NAV_MEMORY))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .child(div().flex_1().min_w_0().child(self.search.clone()))
                    .child(text_button(
                        "memory-clear",
                        theme,
                        copy::CLEAR,
                        theme.primary,
                        true,
                        clear,
                    )),
            );

        if let Some(hint) = hint {
            content = content.child(small(theme, hint.to_string()).p_2());
        }
        if let Some(not_found) = self.model.state().not_found() {
            content = content.child(
                div()
                    .p_2()
                    .text_size(Theme::small_size())
                    .text_color(theme.error)
                    .child(not_found.to_string()),
            );
        }
        content.child(list)
    }
}

/// `RichText` spans in the shape [`styled_text`] wants: `(byte range, bold)`.
fn rich_spans(spans: &[xiaoman_app::chat_item::TextSpan]) -> Vec<(Range<usize>, bool)> {
    spans
        .iter()
        .map(|span| (span.start..span.end, true))
        .collect()
}
