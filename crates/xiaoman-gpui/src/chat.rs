//! The chat screen: the folded transcript, an input row and the approval card.
//!
//! The timeline uses the same `chat_timeline` fold as the desktop app: adjacent
//! progress rows collapse into one 「处理过程」 card, which expands on tap.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, prelude::*, px, rgb, AnyElement, Context, Entity, FontWeight, HighlightStyle, Hsla,
    IntoElement, MouseButton, ParentElement, Render, ScrollHandle, Styled, StyledText, Task,
    TextStyle, Window,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::Disableable;
use xiaoman_api::{ApprovalScope, SkillAction};
use xiaoman_app::chat_item::{chat_rich_text, chat_timeline, ChatItem, TimelineRow};
use xiaoman_app::session::{SendOutcome, Session, SessionSnapshot};

/// The skill draft card's view state: the full `SKILL.md` and what saving will do.
#[derive(Clone, PartialEq)]
struct SkillDraftCard {
    id: i64,
    action: SkillAction,
    name: String,
    content: String,
    previous: Option<String>,
    shadows_bundled: bool,
}

pub struct ChatView {
    session: Option<Arc<Session>>,
    nav: Entity<crate::app::Nav>,
    items: Vec<ChatItem>,
    status: Option<String>,
    approval: Option<(i64, String, String)>,
    /// True between the first approve/deny tap and the card disappearing, so the buttons
    /// cannot be fired twice.
    responding: bool,
    question: Option<(String, bool)>,
    skill_draft: Option<SkillDraftCard>,
    busy: bool,
    /// The expanded 「处理过程」 cards, keyed by their first row's id.
    expanded: HashSet<i64>,
    more_options: bool,
    accessibility: bool,
    notifications: bool,
    input: Entity<TextareaState>,
    scroll: ScrollHandle,
    _subscription: gpui::Subscription,
    _poll: Option<Task<()>>,
}

impl ChatView {
    pub fn new(
        session: Option<Arc<Session>>,
        nav: Entity<crate::app::Nav>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The input grows from one line to four as you type.
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("说说你想做什么…")
                .auto_grow(1, 4)
        });
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::Focus => show_keyboard(),
                InputEvent::Blur => hide_keyboard(),
                InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                _ => cx.notify(),
            });

        let poll = session.clone().map(|session| {
            cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let snapshot = session.state();
                if this
                    .update(cx, |this, cx| {
                        if this.apply(snapshot) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            })
        });

        Self {
            session,
            nav,
            items: Vec::new(),
            status: None,
            approval: None,
            responding: false,
            question: None,
            skill_draft: None,
            busy: false,
            expanded: HashSet::new(),
            more_options: false,
            accessibility: host_accessibility(),
            notifications: host_notifications(),
            input,
            scroll: ScrollHandle::new(),
            _subscription: subscription,
            _poll: poll,
        }
    }

    /// Returns true when something visible changed.
    fn apply(&mut self, snapshot: SessionSnapshot) -> bool {
        // The three banners read the host, not the session; refresh them on the same tick.
        let accessibility = host_accessibility();
        let notifications = host_notifications();
        let host_changed =
            accessibility != self.accessibility || notifications != self.notifications;
        self.accessibility = accessibility;
        self.notifications = notifications;
        let items = snapshot.items.clone();
        let approval = snapshot.approval.as_ref().map(|approval| {
            (
                approval.id,
                approval.prompt.clone(),
                approval.reason.clone(),
            )
        });
        let question = snapshot
            .question
            .as_ref()
            .map(|question| (question.text.clone(), question.takeover));
        let skill_draft = snapshot.skill_draft.as_ref().map(|draft| SkillDraftCard {
            id: draft.id,
            action: draft.action,
            name: draft.name.clone(),
            content: draft.content.clone(),
            previous: draft.previous.clone(),
            shadows_bundled: draft.shadows_bundled,
        });
        let busy = snapshot.busy;
        let status = snapshot.status.clone();
        let changed = items != self.items
            || status != self.status
            || approval != self.approval
            || question != self.question
            || skill_draft != self.skill_draft
            || busy != self.busy;
        if changed {
            // Mirror the two parked prompts and the run state into the shade. The Android host owns the
            // notifications; Rust only names the operation.
            if approval != self.approval {
                self.responding = false;
                match &approval {
                    Some((id, prompt, reason)) => {
                        show_approval(*id, prompt, reason);
                        // The approval card replaces the input row: drop the IME with it.
                        hide_keyboard();
                    }
                    None => hide_approval(),
                }
            }
            if question != self.question {
                match &question {
                    Some((text, takeover)) => show_question(text, *takeover),
                    None => hide_question(),
                }
            }
            if busy != self.busy {
                if busy {
                    start_service();
                } else {
                    stop_service();
                }
            }
            if busy && status != self.status {
                update_status(status.as_deref());
            }
            self.items = items;
            self.status = status;
            self.approval = approval;
            self.question = question;
            self.skill_draft = skill_draft;
            self.busy = busy;
            self.scroll.scroll_to_bottom();
        }
        changed || host_changed
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let outcome = session.send(&text);
        if outcome == SendOutcome::Started {
            start_service();
        }
        // Busy (or a blank prompt) keeps the draft, like the old app.
        if outcome.clears_draft() {
            self.input
                .update(cx, |input, cx| input.set_value("", window, cx));
            window.blur(cx);
            hide_keyboard();
        }
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn approve(&mut self, scope: ApprovalScope, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let Some((id, _, _)) = self.approval else {
            return;
        };
        self.responding = true;
        if hide_app() {
            // The screen the agent must see is the one underneath: move this task out of the way
            // first, like the approval card does, and give the window animator time to finish.
            let session = session.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(650));
                session.approve(id, scope);
            });
        } else {
            session.approve(id, scope);
        }
        self.more_options = false;
        cx.notify();
    }

    fn deny(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = &self.session {
            let Some((id, _, _)) = self.approval else {
                return;
            };
            self.responding = true;
            session.deny(id);
            cx.notify();
        }
    }

    /// The "去设置" banner: switch the shared tab, which the shell observes and repaints.
    fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.nav.update(cx, |nav, cx| {
            nav.tab = crate::app::Tab::Settings;
            cx.notify();
        });
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = &self.session {
            session.stop();
            cx.notify();
        }
    }

    fn answer_skill_draft(&mut self, save: bool, cx: &mut Context<Self>) {
        if let (Some(session), Some(draft)) = (&self.session, &self.skill_draft) {
            session.answer_skill_draft(draft.id, save);
            cx.notify();
        }
    }

    fn item(&self, item: &ChatItem) -> impl IntoElement {
        match item {
            ChatItem::User { text, .. } => div().flex().justify_end().child(
                div()
                    .max_w(px(320.))
                    .p_2()
                    .rounded_lg()
                    .bg(rgb(0x1e6b52))
                    .child(rich_text(text, rgb(0xffffff).into(), 14.)),
            ),
            ChatItem::Assistant { text, .. } => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().text_color(rgb(0x1e6b52)).child("小满"))
                .child(
                    div()
                        .max_w(px(340.))
                        .p_2()
                        .rounded_lg()
                        .bg(rgb(0xffffff))
                        .child(rich_text(text, rgb(0x202c25).into(), 14.)),
                ),
            ChatItem::Progress { text, ok, .. } => div().text_sm().child(rich_text(
                text,
                if *ok == Some(false) {
                    rgb(0xb3261e)
                } else {
                    rgb(0x637168)
                }
                .into(),
                12.,
            )),
            ChatItem::Question { text, takeover, .. } => div()
                .p_2()
                .rounded_lg()
                .bg(rgb(0xe1f0e8))
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().child(if *takeover {
                    "需要你帮一下"
                } else {
                    "再告诉我一点"
                }))
                .child(
                    div()
                        .text_sm()
                        .child(rich_text(text, rgb(0x202c25).into(), 12.)),
                ),
            ChatItem::Error { text, .. } => div()
                .p_2()
                .rounded_lg()
                .bg(rgb(0xfdecea))
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().child("这次没完成"))
                .child(
                    div()
                        .text_sm()
                        .child(rich_text(text, rgb(0xb3261e).into(), 12.)),
                ),
        }
    }

    fn timeline_row(&self, row: &TimelineRow, cx: &mut Context<Self>) -> AnyElement {
        match row {
            TimelineRow::Message(item) => self.item(item).into_any_element(),
            TimelineRow::Process(items) => {
                let key = items.first().map(ChatItem::id).unwrap_or(0);
                let expanded = self.expanded.contains(&key);
                let toggle = div()
                    .id("process-toggle")
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .text_color(rgb(0x637168))
                    .cursor_pointer()
                    .child(format!("处理过程 · {}条记录", items.len()))
                    .child(if expanded { "收起" } else { "展开" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&key) {
                            this.expanded.insert(key);
                        }
                        cx.notify();
                    }));
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(toggle)
                    .when(expanded, |this| {
                        this.children(items.iter().map(|item| self.item(item)))
                    })
                    .into_any_element()
            }
        }
    }
}

fn skill_action_label(action: SkillAction) -> &'static str {
    match action {
        SkillAction::Create => "新建",
        SkillAction::Update => "修改",
        SkillAction::Delete => "删除",
    }
}

/// What the card's preview shows: the new SKILL.md, or the current one for a delete.
fn draft_body(draft: &SkillDraftCard) -> String {
    if draft.content.is_empty() {
        draft.previous.clone().unwrap_or_default()
    } else {
        draft.content.clone()
    }
}

#[cfg(target_os = "android")]
fn start_service() {
    xiaoman_android::session::start_service();
}

#[cfg(not(target_os = "android"))]
fn start_service() {}

#[cfg(target_os = "android")]
fn stop_service() {
    xiaoman_android::session::stop_service();
}

#[cfg(not(target_os = "android"))]
fn stop_service() {}

#[cfg(target_os = "android")]
fn update_status(text: Option<&str>) {
    xiaoman_android::session::update_status(text);
}

#[cfg(not(target_os = "android"))]
fn update_status(_text: Option<&str>) {}

#[cfg(target_os = "android")]
fn show_approval(id: i64, prompt: &str, reason: &str) {
    xiaoman_android::session::show_approval(id, prompt, reason);
}

#[cfg(not(target_os = "android"))]
fn show_approval(_id: i64, _prompt: &str, _reason: &str) {}

#[cfg(target_os = "android")]
fn hide_approval() {
    xiaoman_android::session::hide_approval();
}

#[cfg(not(target_os = "android"))]
fn hide_approval() {}

#[cfg(target_os = "android")]
fn show_question(text: &str, takeover: bool) {
    xiaoman_android::session::show_question(text, takeover);
}

#[cfg(not(target_os = "android"))]
fn show_question(_text: &str, _takeover: bool) {}

#[cfg(target_os = "android")]
fn hide_question() {
    xiaoman_android::session::hide_question();
}

#[cfg(not(target_os = "android"))]
fn hide_question() {}

#[cfg(target_os = "android")]
fn show_keyboard() {
    // The chat input's IME action is Send: the keyboard's send key submits the message.
    gpui_mobile::show_keyboard_with_action(gpui_mobile::ImeAction::Send);
}

#[cfg(not(target_os = "android"))]
fn show_keyboard() {}

#[cfg(target_os = "android")]
fn hide_keyboard() {
    gpui_mobile::hide_keyboard();
}

#[cfg(not(target_os = "android"))]
fn hide_keyboard() {}

impl Render for ChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.clone();
        let configured = session
            .as_ref()
            .map(|session| session.configured())
            .unwrap_or(false);
        let attention =
            self.approval.is_some() || self.question.is_some() || self.skill_draft.is_some();
        let stopping = self.busy && self.question.is_none();
        // The send button is inert until there is something to send, like the old app.
        let can_send = !self.input.read(cx).value().trim().is_empty();
        let ready = configured && self.accessibility;
        let pill = if attention {
            "等你确认"
        } else if self.busy {
            "处理中"
        } else if ready {
            "已就绪"
        } else {
            "待设置"
        };
        let status_line = self
            .status
            .clone()
            .unwrap_or_else(|| "正在想办法…".to_string());
        let timeline = chat_timeline(&self.items);
        let rows: Vec<AnyElement> = timeline
            .iter()
            .map(|row| self.timeline_row(row, cx))
            .collect();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xf7f9f6))
            .text_color(rgb(0x202c25))
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_lg().child("小满"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x637168))
                                    .child("你的手机小助手"),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_lg()
                            .bg(rgb(0xe1f0e8))
                            .child(div().text_xs().text_color(rgb(0x1e6b52)).child(pill)),
                    ),
            )
            .when(!self.accessibility, |this| {
                this.child(banner(
                    "无障碍服务未开启，小满还不能操作其他 App。",
                    Button::new("banner-a11y")
                        .label("去开启")
                        .on_click(cx.listener(|_, _, _, _| open_accessibility_settings())),
                ))
            })
            .when(!configured, |this| {
                this.child(banner(
                    "还没有配置聊天模型和操作模型。",
                    Button::new("banner-models")
                        .label("去设置")
                        .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
                ))
            })
            .when(!self.notifications, |this| {
                this.child(banner(
                    "允许通知后，任务运行和需要确认时会提醒你。",
                    Button::new("banner-notifications")
                        .label("允许")
                        .on_click(cx.listener(|_, _, _, _| request_notifications())),
                ))
            })
            .child(
                div()
                    .id("conversation")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .when(self.items.is_empty(), |this| {
                                this.child(
                                    div()
                                        .py_6()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(div().text_xl().child("今天想做点什么？"))
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(0x637168))
                                                .child("说一句，我来帮你动手。"),
                                        )
                                        .children(EXAMPLES.iter().map(
                                            |(id, title, detail, prompt)| {
                                                div()
                                                    .id(format!("example-{id}"))
                                                    .cursor_pointer()
                                                    .p_3()
                                                    .rounded_lg()
                                                    .bg(rgb(0xffffff))
                                                    .flex()
                                                    .flex_col()
                                                    .gap_1()
                                                    .child(div().text_base().child(*title))
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(rgb(0x637168))
                                                            .child(*detail),
                                                    )
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.input.update(cx, |state, cx| {
                                                                state.set_value(*prompt, window, cx)
                                                            });
                                                            cx.notify();
                                                        },
                                                    ))
                                            },
                                        ))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0x637168))
                                                .child("也可以直接输入你的想法"),
                                        ),
                                )
                            })
                            .children(rows),
                    ),
            )
            .when(
                self.busy && self.approval.is_none() && self.skill_draft.is_none(),
                |this| {
                    this.child(
                        div()
                            .flex_none()
                            .px_4()
                            .py_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_sm().text_color(rgb(0x1e6b52)).child(status_line))
                            .child(
                                Button::new("status-stop")
                                    .label("停止")
                                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                            ),
                    )
                },
            )
            .when_some(self.approval.clone(), |this, (_id, prompt, reason)| {
                this.child(
                    div()
                        .flex_none()
                        .mx_3()
                        .mb_2()
                        .p_3()
                        .rounded_lg()
                        .bg(rgb(0xedf3ee))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_sm().child("这一步，请你确认"))
                        .child(
                            div()
                                .id("approval-prompt")
                                .max_h(px(110.))
                                .overflow_y_scroll()
                                .child(prompt),
                        )
                        .child(div().text_xs().text_color(rgb(0x254735)).child(reason))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    Button::new("deny")
                                        .label("暂不允许")
                                        .disabled(self.responding)
                                        .on_click(cx.listener(|this, _, _, cx| this.deny(cx))),
                                )
                                .child(
                                    Button::new("once")
                                        .label("允许一次")
                                        .disabled(self.responding)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.approve(ApprovalScope::Once, cx)
                                        })),
                                ),
                        )
                        .child(if self.more_options {
                            div()
                                .child(
                                    Button::new("task")
                                        .label("本次任务的同类操作都允许")
                                        .disabled(self.responding)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.approve(ApprovalScope::Task, cx)
                                        })),
                                )
                                .into_any_element()
                        } else {
                            div()
                                .child(Button::new("more").label("更多选项").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.more_options = true;
                                        cx.notify();
                                    },
                                )))
                                .into_any_element()
                        }),
                )
            })
            .when_some(self.skill_draft.clone(), |this, draft| {
                let title = format!("技能草稿 · {}", skill_action_label(draft.action));
                let body = draft_body(&draft);
                this.child(
                    div()
                        .flex_none()
                        .mx_3()
                        .mb_2()
                        .p_3()
                        .rounded_lg()
                        .bg(rgb(0xf4f1e4))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_sm().child(title))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x637168))
                                .child(draft.name.clone()),
                        )
                        .when(draft.shadows_bundled, |this| {
                            this.child(div().text_xs().text_color(rgb(0xb3261e)).child(
                                "将新建用户副本覆盖同名内置技能；内置文件不变，也仍然不能删除",
                            ))
                        })
                        .when(draft.action == SkillAction::Delete, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xb3261e))
                                    .child("将删除用户技能；内置技能不会受影响"),
                            )
                        })
                        .child(
                            div()
                                .id("skill-draft-content")
                                .max_h(px(260.))
                                .overflow_y_scroll()
                                .p_2()
                                .rounded_lg()
                                .bg(rgb(0xffffff))
                                .child(div().text_xs().child(body)),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(Button::new("skill-draft-cancel").label("取消").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.answer_skill_draft(false, cx)
                                    }),
                                ))
                                .child(Button::new("skill-draft-save").label("保存").on_click(
                                    cx.listener(|this, _, _, cx| this.answer_skill_draft(true, cx)),
                                )),
                        ),
                )
            })
            .when(self.question.is_some(), |this| {
                this.child(
                    div().flex_none().px_4().py_1().child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x637168))
                            .child("告诉我就好…"),
                    ),
                )
            })
            .when(
                self.approval.is_none() && self.skill_draft.is_none(),
                |this| {
                    this.child(
                        div().flex_none().p_3().flex().flex_col().gap_2().child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        // A second tap after the IME was dismissed must re-open it:
                                        // the field is still focused, so no Focus event fires again.
                                        .on_mouse_down(MouseButton::Left, |_, _, _| show_keyboard())
                                        .child(Textarea::new(&self.input)),
                                )
                                // One button: send unless a task runs.
                                .child(if stopping {
                                    Button::new("send-stop")
                                        .label("停止")
                                        .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
                                        .into_any_element()
                                } else {
                                    Button::new("send")
                                        .label("发送")
                                        .disabled(!can_send)
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.send(window, cx)
                                            }),
                                        )
                                        .into_any_element()
                                }),
                        ),
                    )
                },
            )
    }
}

/// A chat bubble's body: `chat_rich_text` strips the heading/list markers and returns the byte
/// ranges drawn bold, which map 1:1 onto `StyledText`'s highlighted runs.
fn rich_text(raw: &str, color: Hsla, size: f32) -> StyledText {
    let rich = chat_rich_text(raw);
    let style = TextStyle {
        color,
        font_size: px(size).into(),
        ..TextStyle::default()
    };
    let highlights = rich.spans.iter().map(|span| {
        (
            span.start..span.end,
            HighlightStyle {
                font_weight: Some(FontWeight::SEMIBOLD),
                ..HighlightStyle::default()
            },
        )
    });
    StyledText::new(rich.text).with_default_highlights(&style, highlights)
}

/// The three starting points of the welcome screen: title, detail, prompt.
const EXAMPLES: [(&str, &str, &str, &str); 3] = [
    (
        "shopping",
        "选一本笔记本",
        "20元以内，先给我确认",
        "帮我在淘宝挑一本20元以内的A5笔记本，选好后给我确认。",
    ),
    (
        "water",
        "提醒我喝水",
        "10分钟后提醒",
        "10分钟后提醒我喝水。",
    ),
    (
        "prefs",
        "记住我的偏好",
        "让助手慢慢懂你",
        "记住我买东西喜欢简单实用。",
    ),
];

/// A one-line notice above the transcript with its own action button.
fn banner(text: &str, action: Button) -> impl IntoElement {
    div()
        .flex_none()
        .px_4()
        .py_1()
        .bg(rgb(0xedf3ee))
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(div().text_xs().child(text.to_string()))
        .child(action)
}

#[cfg(target_os = "android")]
fn host_accessibility() -> bool {
    xiaoman_android::session::accessibility_enabled()
}

#[cfg(not(target_os = "android"))]
fn host_accessibility() -> bool {
    true
}

#[cfg(target_os = "android")]
fn host_notifications() -> bool {
    xiaoman_android::session::notifications_allowed()
}

#[cfg(not(target_os = "android"))]
fn host_notifications() -> bool {
    true
}

#[cfg(target_os = "android")]
fn request_notifications() {
    xiaoman_android::session::request_notifications();
}

#[cfg(not(target_os = "android"))]
fn request_notifications() {}

#[cfg(target_os = "android")]
fn open_accessibility_settings() {
    xiaoman_android::session::open_accessibility_settings();
}

#[cfg(not(target_os = "android"))]
fn open_accessibility_settings() {}

#[cfg(target_os = "android")]
fn hide_app() -> bool {
    xiaoman_android::session::hide_app()
}

#[cfg(not(target_os = "android"))]
fn hide_app() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(action: SkillAction, content: &str, previous: Option<&str>) -> SkillDraftCard {
        SkillDraftCard {
            id: 1,
            action,
            name: "daily-review".to_string(),
            content: content.to_string(),
            previous: previous.map(str::to_string),
            shadows_bundled: false,
        }
    }

    #[test]
    fn draft_titles_name_the_action() {
        assert_eq!(skill_action_label(SkillAction::Create), "新建");
        assert_eq!(skill_action_label(SkillAction::Update), "修改");
        assert_eq!(skill_action_label(SkillAction::Delete), "删除");
    }

    #[test]
    fn draft_body_falls_back_to_the_current_skill_for_delete() {
        let draft = card(SkillAction::Delete, "", Some("当前内容"));
        assert_eq!(draft_body(&draft), "当前内容");
        let draft = card(SkillAction::Create, "新内容", None);
        assert_eq!(draft_body(&draft), "新内容");
    }
}
