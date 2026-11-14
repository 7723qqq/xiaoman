//! `SkillsScreen.kt` in GPUI: the reload button, the skill cards with their flow indicator, and the
//! loading problems below them.
//!
//! The cards come from the session's `DirectorySkillRepository`, so a skill folder dropped next to
//! the settings shows up after 重新加载.

use gpui::{
    ClickEvent, Context, Div, IntoElement, Render, ScrollHandle, Window, div, prelude::*, px,
};

use xiaoman_app::session::Session;

use crate::theme::{Theme, theme};
use crate::view_model::copy;
use crate::view_model::skills::{ProblemRow, SkillRow, SkillsModel};

use super::components::{
    card, column, divider, heading, label, outlined_button, row, small, title, title_small,
};

/// The skills screen.
pub struct SkillsView {
    model: SkillsModel,
    session: Session,
    scroll: ScrollHandle,
}

impl SkillsView {
    /// The skills the session loaded from its data folder; the cards start with what is there.
    pub fn new(cx: &mut Context<Self>, session: Session) -> Self {
        let _ = cx;
        let mut model = SkillsModel::new().with_folder(session.skills_dir().display().to_string());
        let result = session.skills_reload();
        model.set_result(&result);
        Self {
            model,
            session,
            scroll: ScrollHandle::new(),
        }
    }

    pub fn model(&self) -> &SkillsModel {
        &self.model
    }

    /// `repository.reload()` on the session's folder.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.model.reload();
        let result = self.session.skills_reload();
        self.model.set_result(&result);
        cx.notify();
    }

    fn skill_card(theme: &Theme, skill: &SkillRow) -> Div {
        let mut content = column()
            .gap_1()
            .p_3()
            .child(title_small(theme, skill.name.clone()))
            .child(
                div()
                    .text_size(Theme::body_size())
                    .line_height(Theme::body_line_height())
                    .text_color(theme.on_surface)
                    .child(skill.description.clone()),
            );
        if let Some(apps) = &skill.apps {
            content = content.child(small(theme, apps.clone()));
        }
        content = content.child(label(theme, skill.source.clone(), theme.on_surface_variant));
        if let Some(flow) = &skill.flow {
            content = content.child(
                div()
                    .mt_1()
                    .px_2()
                    .py_1()
                    .rounded_full()
                    .bg(theme.primary_container)
                    .text_size(Theme::label_size())
                    .text_color(theme.on_primary_container)
                    .child(flow.label.clone()),
            );
        }
        div().w_full().child(card(theme).child(content))
    }

    fn problem_card(theme: &Theme, problem: &ProblemRow) -> Div {
        div().w_full().child(
            column()
                .gap_1()
                .p_3()
                .rounded_lg()
                .bg(theme.error_container)
                .child(
                    div()
                        .text_size(Theme::label_size())
                        .text_color(theme.on_error_container)
                        .child(problem.source.clone()),
                )
                .child(
                    div()
                        .text_size(Theme::body_size())
                        .text_color(theme.on_error_container)
                        .child(problem.message.clone()),
                ),
        )
    }
}

impl Render for SkillsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let loading = self.model.loading();
        let reload = cx.listener(|this, _event: &ClickEvent, _window, cx| {
            this.reload(cx);
        });

        let mut list = column().gap_2();
        for skill in self.model.rows() {
            list = list.child(Self::skill_card(&theme, skill));
        }
        if let Some(hint) = self.model.empty_hint() {
            list = list.child(small(&theme, hint).p_2());
        }
        if !self.model.problems().is_empty() {
            list = list
                .child(divider(&theme))
                .child(title(&theme, copy::SKILLS_PROBLEMS));
            for problem in self.model.problems() {
                list = list.child(Self::problem_card(&theme, problem));
            }
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .child(
                row()
                    .w_full()
                    .px_4()
                    .py_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(heading(&theme, copy::SKILLS_TITLE)),
                    )
                    .child(outlined_button(
                        "skills-reload",
                        &theme,
                        copy::SKILLS_RELOAD,
                        !loading,
                        reload,
                    )),
            )
            .child(
                div()
                    .id("skills-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(px(16.))
                    .child(list),
            )
    }
}
