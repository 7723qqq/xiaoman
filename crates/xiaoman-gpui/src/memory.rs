//! Memory: the note list, Chinese search, one note open at a time and real deletion.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, prelude::*, rgb, Context, Entity, IntoElement, ParentElement, Render, Styled, Task, Window,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use xiaoman_api::Note;
use xiaoman_app::session::Session;
use xiaoman_memory::BasicMemoryFormat;

use crate::ui::{hint, input, title, value};

pub struct MemoryView {
    session: Option<Arc<Session>>,
    notes: Vec<Note>,
    query: Entity<InputState>,
    open: Option<Note>,
    /// The permalink the user asked to forget; deletion waits for the second tap.
    confirm_delete: Option<String>,
    /// True when the list holds search hits rather than every note.
    searched: bool,
    message: Option<String>,
    /// The pending 250 ms debounce behind typing, with a 250 ms debounce.
    debounce: Option<Task<()>>,
    _subscription: gpui::Subscription,
}

impl MemoryView {
    pub fn new(session: Option<Arc<Session>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let notes = session
            .as_ref()
            .and_then(|session| session.memory_list(None).ok())
            .unwrap_or_default();
        let query = input(window, cx, "搜索记忆", "");
        let subscription = cx.subscribe_in(&query, window, |this, _, event, _, cx| {
            if let InputEvent::Change = event {
                this.schedule_search(cx);
            }
        });
        Self {
            session,
            notes,
            query,
            open: None,
            confirm_delete: None,
            searched: false,
            message: None,
            debounce: None,
            _subscription: subscription,
        }
    }

    /// Types pause, then the list becomes the search hits of whatever is in the field now.
    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let typed = value(&self.query, cx);
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let _ = this.update(cx, |this, cx| {
                if value(&this.query, cx) == typed {
                    this.search(cx);
                }
            });
        }));
    }

    /// The system back button: closes the open note (and its delete confirmation).
    /// Returns true when it handled the back press.
    pub fn back(&mut self, cx: &mut Context<Self>) -> bool {
        if self.open.is_some() {
            self.open = None;
            self.confirm_delete = None;
            cx.notify();
            true
        } else {
            false
        }
    }

    fn list(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        match session.memory_list(None) {
            Ok(notes) => {
                self.notes = notes;
                self.searched = false;
                self.message = None;
            }
            Err(error) => self.message = Some(format!("读取失败：{error}")),
        }
        cx.notify();
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let query = value(&self.query, cx);
        if query.is_empty() {
            self.list(cx);
            return;
        }
        match session.memory_search(&query, 50) {
            Ok(hits) => {
                self.notes = hits
                    .into_iter()
                    .map(|hit| Note {
                        title: hit.title,
                        permalink: hit.permalink,
                        body: hit.snippet,
                        ..Note::new(String::new(), String::new())
                    })
                    .collect();
                self.searched = true;
                self.message = Some(format!("找到 {} 条", self.notes.len()));
            }
            Err(error) => self.message = Some(format!("搜索失败：{error}")),
        }
        cx.notify();
    }

    fn open(&mut self, permalink: &str, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        self.confirm_delete = None;
        match session.memory_read(permalink) {
            Ok(Some(note)) => self.open = Some(note),
            Ok(None) => self.message = Some("这条笔记已不存在。".to_string()),
            Err(error) => self.message = Some(format!("读取失败：{error}")),
        }
        cx.notify();
    }

    fn delete(&mut self, permalink: &str, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        self.confirm_delete = None;
        match session.memory_delete(permalink) {
            Ok(true) => {
                self.open = None;
                self.message = Some("已删除。".to_string());
                self.list(cx);
            }
            Ok(false) => self.message = Some("删除失败：文件不存在。".to_string()),
            Err(error) => self.message = Some(format!("删除失败：{error}")),
        }
        cx.notify();
    }
}

impl Render for MemoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(note) = self.open.clone() {
            return div().size_full().id("note").overflow_y_scroll().child(
                div()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(title(&note.title))
                            .child(
                                Button::new("close-note")
                                    .label("返回")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(hint(note.permalink.clone()))
                    .child(match self.confirm_delete.as_deref() {
                        Some(pending) if pending == note.permalink.as_str() => div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .rounded_lg()
                            .bg(rgb(0xfdecea))
                            .child(div().text_base().child("删除这条笔记？"))
                            .child(hint(format!("「{}」会从手机上永久删除。", note.title)))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(Button::new("cancel-delete").label("取消").on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.confirm_delete = None;
                                            cx.notify();
                                        }),
                                    ))
                                    .child(
                                        Button::new("confirm-delete")
                                            .danger()
                                            .label("删除")
                                            .on_click(cx.listener({
                                                let permalink = note.permalink.clone();
                                                move |this, _, _, cx| this.delete(&permalink, cx)
                                            })),
                                    ),
                            )
                            .into_any_element(),
                        _ => Button::new("delete-note")
                            .danger()
                            .label("删除")
                            .on_click(cx.listener({
                                let permalink = note.permalink.clone();
                                move |this, _, _, cx| {
                                    this.confirm_delete = Some(permalink.clone());
                                    cx.notify();
                                }
                            }))
                            .into_any_element(),
                    })
                    .child(div().text_sm().child(BasicMemoryFormat::render(&note))),
            );
        }

        div().size_full().id("memory").overflow_y_scroll().child(
            div()
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(title("记忆"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(Input::new(&self.query).flex_1().cleanable(true))
                        .child(
                            Button::new("search-memory")
                                .primary()
                                .label("搜索")
                                .on_click(cx.listener(|this, _, _, cx| this.search(cx))),
                        )
                        .child(
                            Button::new("list-memory")
                                .label("全部")
                                .on_click(cx.listener(|this, _, _, cx| this.list(cx))),
                        ),
                )
                .when_some(self.message.clone(), |this, message| {
                    this.child(div().text_sm().child(message))
                })
                .when(self.notes.is_empty(), |this| {
                    this.child(hint(if self.searched {
                        "没有找到相关笔记。"
                    } else {
                        "还没有记忆。"
                    }))
                })
                .children(self.notes.iter().map(|note| {
                    let permalink = note.permalink.clone();
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .rounded_lg()
                        .bg(rgb(0xffffff))
                        // The whole row opens the note, like the old app's clickable ListItem.
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.open(&permalink, cx);
                            }),
                        )
                        .child(div().text_base().child(note.title.clone()))
                        .child(hint(note.permalink.clone()))
                        // Search hits carry the matching snippet in `body`.
                        .when(self.searched && !note.body.is_empty(), |this| {
                            this.child(hint(note.body.clone()))
                        })
                })),
        )
    }
}
