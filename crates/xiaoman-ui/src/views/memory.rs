//! `MemoryScreen.kt` in GPUI: the search box, the list of notes, and the empty states.
//!
//! The list and the search run against the session's `FileMemoryStore`; every refresh replaces the
//! rows with what is on disk now, so a note the agent wrote appears without a restart.

use gpui::{ClickEvent, Context, Entity, IntoElement, Render, Window, div, prelude::*};

use xiaoman_app::session::Session;

use crate::theme::{Theme, theme};
use crate::view_model::copy;
use crate::view_model::memory::{MemoryModel, MemoryRow};

use super::components::{column, heading, small, text_button};
use super::text_field::{FieldChanged, TextField};

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

    fn row_card(theme: &Theme, row: &MemoryRow) -> impl IntoElement {
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
            .child(
                div()
                    .text_size(Theme::body_size())
                    .text_color(theme.on_surface)
                    .child(row.title.clone()),
            )
            .child(small(theme, row.detail.clone()))
    }
}

impl Render for MemoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let clear = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.model.set_query("");
            this.search.update(cx, |input, cx| input.clear(cx));
            this.reload(cx);
        });

        let rows = self.model.rows().to_vec();
        let hint = self.model.state().empty_hint();
        let not_found = self.model.state().not_found();

        let mut list = column().gap_2();
        for row in &rows {
            list = list.child(Self::row_card(&theme, row));
        }

        let mut content = column()
            .gap_2()
            .p_3()
            .child(heading(&theme, copy::NAV_MEMORY))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .child(div().flex_1().min_w_0().child(self.search.clone()))
                    .child(text_button(&theme, copy::CLEAR, theme.primary, true, clear)),
            );

        if let Some(hint) = hint {
            content = content.child(small(&theme, hint.to_string()).p_2());
        }
        if let Some(not_found) = not_found {
            content = content.child(
                div()
                    .p_2()
                    .text_size(Theme::small_size())
                    .text_color(theme.error)
                    .child(not_found.to_string()),
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
                    .child(content.child(list)),
            )
    }
}
