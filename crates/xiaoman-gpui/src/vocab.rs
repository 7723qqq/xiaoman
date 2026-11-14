//! The 背单词 page: today's reviews, today's new words and the study streak.

use std::sync::Arc;

use gpui::{
    div, prelude::*, rgb, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use xiaoman_api::{VocabDueWord, VocabPrefs, VocabToday, VocabWord};
use xiaoman_app::session::Session;

use crate::app::{Nav, Tab};
use crate::ui::{hint, title};

pub struct VocabView {
    session: Option<Arc<Session>>,
    nav: Entity<Nav>,
    plan: Option<VocabToday>,
    message: Option<String>,
}

impl VocabView {
    pub fn new(session: Option<Arc<Session>>, nav: Entity<Nav>, _cx: &mut Context<Self>) -> Self {
        // Loading waits until the tab is opened, so opening the app does not consume the day's
        // words or count a study day by itself.
        Self {
            session,
            nav,
            plan: None,
            message: None,
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            self.plan = None;
            self.message = Some("还没有配置模型，先在设置里填好模型信息。".to_string());
            cx.notify();
            return;
        };
        let Some(vocab) = session.vocab() else {
            self.plan = None;
            self.message = Some("背单词引擎不可用。".to_string());
            cx.notify();
            return;
        };
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        match vocab.today(&today) {
            Ok(plan) => {
                self.plan = Some(plan);
                self.message = None;
            }
            Err(error) => {
                self.plan = None;
                self.message = Some(format!("读取背单词计划失败：{error}"));
            }
        }
        cx.notify();
    }

    fn stat(value: String, label: &str) -> impl IntoElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .p_3()
            .rounded_lg()
            .bg(rgb(0xffffff))
            .child(div().text_xl().child(value))
            .child(hint(label))
    }

    fn word_card(word: &VocabWord) -> gpui::Div {
        let mut card = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_lg()
            .bg(rgb(0xffffff))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(div().text_lg().child(word.word.clone()))
                    .when(!word.phonetic.trim().is_empty(), |this| {
                        this.child(hint(format!("/{}/", word.phonetic)))
                    }),
            )
            .child(div().text_sm().child(meaning_line(word)));
        if !word.example.trim().is_empty() {
            card = card.child(div().text_sm().child(word.example.clone()));
        }
        if !word.example_zh.trim().is_empty() {
            card = card.child(hint(word.example_zh.clone()));
        }
        for pair in &word.collocations {
            card = card.child(hint(format!("{}：{}", pair.en, pair.zh)));
        }
        card
    }

    fn due_card(due: &VocabDueWord) -> gpui::Div {
        Self::word_card(&due.word).child(hint(format!(
            "第 {} 轮 · 到期 {} · 答错 {} 次",
            due.stage + 1,
            due.due,
            due.wrong
        )))
    }

    fn prefs_line(prefs: &VocabPrefs) -> String {
        let reminder = if prefs.reminder_enabled {
            format!("{} 提醒", prefs.reminder_time)
        } else {
            "提醒未开启".to_string()
        };
        format!("每天 {} 个新词 · {}", prefs.daily_new, reminder)
    }
}

fn meaning_line(word: &VocabWord) -> String {
    word.display_meaning()
}

impl Render for VocabView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reload = cx.listener(|this, _, _, cx| this.reload(cx));
        let to_chat = cx.listener(|this, _, _, cx| {
            this.nav.update(cx, |nav, cx| {
                nav.tab = Tab::Chat;
                cx.notify();
            });
            cx.notify();
        });

        let mut page = div().size_full().id("vocab").overflow_y_scroll().child(
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
                        .child(title("背单词"))
                        .child(Button::new("vocab-refresh").label("刷新").on_click(reload)),
                )
                .when_some(self.message.clone(), |this, message| {
                    this.child(div().text_sm().child(message))
                }),
        );

        if let Some(plan) = self.plan.clone() {
            let stats = plan.stats.clone();
            page = page.child(
                div()
                    .p_4()
                    .pt_0()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(hint(format!(
                        "{} · {} 个到期复习 · 词库还剩 {} 个",
                        plan.date, plan.due_total, plan.remaining
                    )))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(Self::stat(stats.learned.to_string(), "已学"))
                            .child(Self::stat(stats.due_today.to_string(), "到期"))
                            .child(Self::stat(stats.streak_days.to_string(), "连续天"))
                            .child(Self::stat(stats.mastered.to_string(), "已掌握")),
                    )
                    .child(hint(Self::prefs_line(&plan.prefs)))
                    .child(
                        div()
                            .text_base()
                            .child(format!("今天要复习（{}）", plan.due_total)),
                    )
                    .when(plan.due_words.is_empty(), |this| {
                        this.child(hint("今天没有到期复习，可以直接看新词。"))
                    })
                    .when(plan.due_total as usize > plan.due_words.len(), |this| {
                        this.child(hint(format!(
                            "还有 {} 个到期词，背完这批再刷新。",
                            plan.due_total as usize - plan.due_words.len()
                        )))
                    })
                    .children(plan.due_words.iter().map(Self::due_card))
                    .child(
                        div()
                            .text_base()
                            .child(format!("今天的新词（{}）", plan.new_words.len())),
                    )
                    .when(plan.new_words.is_empty(), |this| {
                        this.child(hint(if plan.remaining == 0 {
                            "词库已经背完了，可以在聊天里用 vocab_add_word 添加新词。"
                        } else {
                            "今天的新词已经生成过了，明天再来。"
                        }))
                    })
                    .children(plan.new_words.iter().map(Self::word_card))
                    .child(
                        Button::new("vocab-to-chat")
                            .primary()
                            .label("去聊天背单词")
                            .on_click(to_chat),
                    ),
            );
        }

        page
    }
}
