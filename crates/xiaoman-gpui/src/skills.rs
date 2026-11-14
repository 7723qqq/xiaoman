//! Skills: what is loaded, where it comes from, problems, and the reload button.

use std::sync::Arc;

use gpui::{div, prelude::*, rgb, Context, IntoElement, ParentElement, Render, Styled, Window};
use gpui_kit::component::button::{Button, ButtonVariants};
use xiaoman_api::{Skill, SkillProblem};
use xiaoman_app::session::Session;

use crate::ui::{hint, title};

pub struct SkillsView {
    session: Option<Arc<Session>>,
    skills: Vec<Skill>,
    problems: Vec<SkillProblem>,
    message: Option<String>,
    loading: bool,
}

impl SkillsView {
    pub fn new(session: Option<Arc<Session>>, cx: &mut Context<Self>) -> Self {
        let (skills, problems) = first_load(session.as_ref());
        let _ = cx;
        Self {
            session,
            skills,
            problems,
            message: None,
            loading: false,
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            self.message = Some("会话还没有打开。".to_string());
            cx.notify();
            return;
        };
        self.loading = true;
        cx.notify();
        let result = session.skills_reload();
        self.message = Some(format!(
            "已加载 {} 个技能{}",
            result.skills.len(),
            if result.problems.is_empty() {
                String::new()
            } else {
                format!("，{} 个有问题", result.problems.len())
            }
        ));
        self.skills = result.skills;
        self.problems = result.problems;
        self.loading = false;
        cx.notify();
    }
}

impl Render for SkillsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let skills_dir = self
            .session
            .as_ref()
            .map(|session| session.skills_dir().display().to_string())
            .unwrap_or_default();
        div().size_full().id("skills").overflow_y_scroll().child(
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
                        .child(title("技能"))
                        .child(
                            Button::new("reload-skills")
                                .primary()
                                .label("重新加载")
                                .loading(self.loading)
                                .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                        ),
                )
                .when(self.loading, |this| {
                    this.child(
                        gpui_kit::component::progress::Progress::new("skills-progress")
                            .loading(true)
                            .w_full(),
                    )
                })
                .child(hint("放在数据目录的 skills/ 下，或使用内置示例。"))
                .when_some(self.message.clone(), |this, message| {
                    this.child(div().text_sm().child(message))
                })
                .when(self.skills.is_empty(), |this| {
                    this.child(hint(format!(
                        "还没有技能。把技能文件夹放到 {skills_dir} 后点重新加载。"
                    )))
                })
                .children(self.skills.iter().map(|skill| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .rounded_lg()
                        .bg(rgb(0xffffff))
                        .child(div().text_base().child(skill.name.clone()))
                        .child(div().text_sm().child(skill.description.clone()))
                        .when(!skill.apps.is_empty(), |this| {
                            this.child(hint(format!("App：{}", skill.apps.join(", "))))
                        })
                        .child(hint(skill.source.clone()))
                        .child(hint(format!(
                            "{} · {} 个应用 · {} 个确认词 · {} 个流程步骤",
                            skill.channel.wire(),
                            skill.apps.len(),
                            skill.confirm_before.len(),
                            skill
                                .flow
                                .as_ref()
                                .map(|flow| flow.steps.len())
                                .unwrap_or(0),
                        )))
                }))
                .when(!self.problems.is_empty(), |this| {
                    this.child(div().text_sm().text_color(rgb(0xb3261e)).child("加载问题"))
                        .children(self.problems.iter().map(|problem| {
                            div().text_sm().text_color(rgb(0xb3261e)).child(format!(
                                "{}{} {}",
                                problem.source,
                                problem
                                    .line
                                    .map(|line| format!(":{line}"))
                                    .unwrap_or_default(),
                                problem.message
                            ))
                        }))
                }),
        )
    }
}

/// The first paint shows the problems the startup load found, not just the skills.
fn first_load(session: Option<&Arc<Session>>) -> (Vec<Skill>, Vec<SkillProblem>) {
    session
        .map(|session| {
            let result = session.skills_reload();
            (result.skills, result.problems)
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use xiaoman_app::session::{Session, SessionConfig};

    use super::first_load;

    #[test]
    fn the_first_load_carries_the_parse_problems() {
        let dir = tempfile::tempdir().expect("temp dir");
        let skill = dir.path().join("skills/badname");
        std::fs::create_dir_all(&skill).expect("skill dir");
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: Bad Name\ndescription: x\n---\n# Bad\n",
        )
        .expect("skill file");
        let session = Session::new(SessionConfig::new(dir.path())).expect("session");
        let (_skills, problems) = first_load(Some(&Arc::new(session)));
        assert!(
            problems
                .iter()
                .any(|problem| problem.message.contains("must match")),
            "{problems:?}"
        );
    }
}
