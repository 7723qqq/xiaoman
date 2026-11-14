//! `MemoryScreen.kt` in GPUI: the search box, the note list, one open note with its delete
//! confirmation, and the empty states.
//!
//! The list, the note and the delete all run against the session's `FileMemoryStore`; every
//! refresh replaces the rows with what is on disk now, so a note the agent wrote appears without a
//! restart.

use std::ops::Range;
use std::time::Duration;

use gpui::{ClickEvent, Context, Div, Entity, IntoElement, Render, Task, Window, div, prelude::*};

use xiaoman_app::session::Session;

use crate::theme::{Theme, theme};
use crate::view_model::copy;
use crate::view_model::memory::{MemoryModel, MemoryRow};

use super::components::{column, heading, row, small, text_button};
use super::text_field::{
    Input, InputEvent, TextField, field_value, set_field_value, styled_text, text_field,
};

/// What one background load produced.
enum Loaded {
    Notes(Vec<xiaoman_api::Note>),
    Hits(Vec<xiaoman_api::SearchHit>),
}

/// The memory screen.
pub struct MemoryView {
    model: MemoryModel,
    session: Session,
    search: Entity<TextField>,
    /// The pending debounced search; replacing it drops the older one.
    pending: Option<Task<()>>,
    /// Bumped per search, so a slow reply cannot overwrite a newer one.
    generation: u64,
}

impl MemoryView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, session: Session) -> Self {
        let model = MemoryModel::new();
        let search = text_field(window, cx, copy::MEMORY_SEARCH_HINT, "");
        cx.subscribe_in(
            &search,
            window,
            |this, field, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.model.set_query(field_value(field, cx));
                    this.refresh(cx);
                }
            },
        )
        .detach();

        let mut view = Self {
            model,
            session,
            search,
            pending: None,
            generation: 0,
        };
        view.reload(cx);
        view
    }

    pub fn model(&self) -> &MemoryModel {
        &self.model
    }

    /// `memory.list()` for a blank query, `memory.search(q, 50)` otherwise — on a background
    /// thread, because both are blocking file reads and the search box fires per keystroke.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.schedule(cx, Duration::ZERO);
    }

    /// The query changed: wait out the typing before searching.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.schedule(cx, Duration::from_millis(150));
    }

    fn schedule(&mut self, cx: &mut Context<Self>, delay: Duration) {
        self.generation += 1;
        let generation = self.generation;
        let session = self.session.clone();
        let query = self.model.query().trim().to_string();
        self.model.begin_refresh();
        self.pending = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    if query.is_empty() {
                        session.memory_list(None).map(Loaded::Notes)
                    } else {
                        session.memory_search(&query, 50).map(Loaded::Hits)
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    // A newer keystroke already scheduled its own search.
                    return;
                }
                match loaded {
                    Ok(Loaded::Notes(notes)) => this.model.set_notes(&notes),
                    Ok(Loaded::Hits(hits)) => this.model.set_hits(&hits),
                    Err(error) => {
                        eprintln!("读取记忆失败：{error}");
                        this.model.set_failed();
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
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
        let clear = cx.listener(|this, _event: &ClickEvent, window, cx| {
            this.model.set_query("");
            set_field_value(&this.search, "", window, cx);
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
                    .child(div().flex_1().min_w_0().child(Input::new(&self.search)))
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
