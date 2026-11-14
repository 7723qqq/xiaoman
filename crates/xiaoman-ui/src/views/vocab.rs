//! The 背单词 screen: today's due reviews, today's new words and the study streak.

use gpui::{ClickEvent, Context, IntoElement, Render, Window, div, prelude::*};

use xiaoman_api::{VocabDueWord, VocabToday, VocabWord};
use xiaoman_app::session::Session;

use crate::theme::{Theme, theme};
use crate::view_model::copy;

use super::components::{card, column, filled_button, heading, small};

pub struct VocabView {
    session: Session,
    plan: Option<VocabToday>,
    message: Option<String>,
}

impl VocabView {
    pub fn new(_cx: &mut Context<Self>, session: Session) -> Self {
        // Loading waits until the tab is opened, so opening the app does not consume the day's
        // words or count a study day by itself.
        Self {
            session,
            plan: None,
            message: None,
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(vocab) = self.session.vocab() else {
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

    fn word_card(theme: &Theme, word: &VocabWord) -> gpui::Div {
        let meaning = word.display_meaning();
        let mut body = card(theme)
            .p_3()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .text_size(Theme::title_size())
                            .text_color(theme.on_surface)
                            .child(word.word.clone()),
                    )
                    .when(!word.phonetic.trim().is_empty(), |this| {
                        this.child(small(theme, format!("/{}/", word.phonetic)))
                    }),
            )
            .child(
                div()
                    .text_size(Theme::body_size())
                    .text_color(theme.on_surface)
                    .child(meaning),
            );
        if !word.example.trim().is_empty() {
            body = body.child(small(theme, word.example.clone()));
        }
        if !word.example_zh.trim().is_empty() {
            body = body.child(small(theme, word.example_zh.clone()));
        }
        for pair in &word.collocations {
            body = body.child(small(theme, format!("{}：{}", pair.en, pair.zh)));
        }
        body
    }

    fn stat(theme: &Theme, value: String, label: &str) -> impl IntoElement {
        card(theme)
            .flex_1()
            .items_center()
            .p_2()
            .gap_1()
            .child(
                div()
                    .text_size(Theme::title_size())
                    .text_color(theme.on_surface)
                    .child(value),
            )
            .child(small(theme, label.to_string()))
    }
}

impl Render for VocabView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let refresh = cx.listener(|this, _: &ClickEvent, _window, cx| this.reload(cx));

        let mut content = column().gap_2().p_3().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(heading(&theme, copy::NAV_VOCAB))
                .child(filled_button(&theme, copy::VOCAB_REFRESH, true, refresh)),
        );

        if let Some(message) = &self.message {
            content = content.child(small(&theme, message.clone()));
        }

        if let Some(plan) = &self.plan {
            let prefs = if plan.prefs.reminder_enabled {
                format!(
                    "每天 {} 个新词 · {} 提醒",
                    plan.prefs.daily_new, plan.prefs.reminder_time
                )
            } else {
                format!("每天 {} 个新词 · 提醒未开启", plan.prefs.daily_new)
            };
            content = content
                .child(small(
                    &theme,
                    format!(
                        "{} · {} 个到期复习 · 词库还剩 {} 个",
                        plan.date, plan.due_total, plan.remaining
                    ),
                ))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(Self::stat(&theme, plan.stats.learned.to_string(), "已学"))
                        .child(Self::stat(&theme, plan.stats.due_today.to_string(), "到期"))
                        .child(Self::stat(
                            &theme,
                            plan.stats.streak_days.to_string(),
                            "连续天",
                        ))
                        .child(Self::stat(
                            &theme,
                            plan.stats.mastered.to_string(),
                            "已掌握",
                        )),
                )
                .child(small(&theme, prefs))
                .child(heading(&theme, format!("今天要复习（{}）", plan.due_total)));
            if plan.due_words.is_empty() {
                content = content.child(small(&theme, "今天没有到期复习，可以直接看新词。"));
            }
            if plan.due_total as usize > plan.due_words.len() {
                content = content.child(small(
                    &theme,
                    format!(
                        "还有 {} 个到期词，背完这批再刷新。",
                        plan.due_total as usize - plan.due_words.len()
                    ),
                ));
            }
            for due in &plan.due_words {
                content = content
                    .child(Self::word_card(&theme, &due.word).child(small(&theme, due_line(due))));
            }
            content = content.child(heading(
                &theme,
                format!("今天的新词（{}）", plan.new_words.len()),
            ));
            if plan.new_words.is_empty() {
                content = content.child(small(
                    &theme,
                    if plan.remaining == 0 {
                        "词库已经背完了，可以在聊天里添加新词。"
                    } else {
                        "今天的新词已经生成过了，明天再来。"
                    },
                ));
            }
            for word in &plan.new_words {
                content = content.child(Self::word_card(&theme, word));
            }
            content = content.child(small(
                &theme,
                "在聊天里说「背单词」，小满会逐张讲解并记录复习结果。",
            ));
        }

        div()
            .size_full()
            .id("vocab")
            .overflow_y_scroll()
            .child(content)
    }
}

fn due_line(due: &VocabDueWord) -> String {
    format!(
        "第 {} 轮 · 到期 {} · 答错 {} 次",
        due.stage + 1,
        due.due,
        due.wrong
    )
}
