//! The five-tab shell, mirroring the desktop `xiaoman-ui` shell and the Compose host.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, svg, AnyElement, Context, Entity, IntoElement, ParentElement, Render,
    Styled, Window,
};
use xiaoman_app::session::Session;

use crate::chat::ChatView;
use crate::memory::MemoryView;
use crate::services::ServicesView;
use crate::settings::SettingsView;
use crate::skills::SkillsView;

gpui::actions!(xiaoman, [Back]);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    #[default]
    Chat,
    Skills,
    Services,
    Memory,
    Settings,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Chat,
        Tab::Skills,
        Tab::Services,
        Tab::Memory,
        Tab::Settings,
    ];

    /// The bar icon, the five outlined Material icons of the Compose navigation bar.
    fn icon(self) -> &'static [u8] {
        match self {
            Self::Chat => include_bytes!("assets/icons/chat.svg"),
            Self::Skills => include_bytes!("assets/icons/skills.svg"),
            Self::Services => include_bytes!("assets/icons/services.svg"),
            Self::Memory => include_bytes!("assets/icons/memory.svg"),
            Self::Settings => include_bytes!("assets/icons/settings.svg"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Chat => "聊天",
            Self::Skills => "技能",
            Self::Services => "服务",
            Self::Memory => "记忆",
            Self::Settings => "设置",
        }
    }
}

/// The selected tab, shared so a screen can send the user to another one (the chat banners).
#[derive(Default)]
pub(crate) struct Nav {
    pub tab: Tab,
}

pub struct RootView {
    nav: Entity<Nav>,
    chat: Entity<ChatView>,
    skills: Entity<SkillsView>,
    services: Entity<ServicesView>,
    memory: Entity<MemoryView>,
    settings: Entity<SettingsView>,
}

impl RootView {
    pub fn new(session: Option<Arc<Session>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|_| Nav::default());
        let chat = cx.new(|cx| ChatView::new(session.clone(), nav.clone(), window, cx));
        let skills = cx.new(|cx| SkillsView::new(session.clone(), cx));
        let services = cx.new(|cx| ServicesView::new(session.clone(), window, cx));
        let memory = cx.new(|cx| MemoryView::new(session.clone(), window, cx));
        let settings = cx.new(|cx| SettingsView::new(session.clone(), window, cx));
        // A banner may switch the tab from outside this view.
        cx.observe(&nav, |_, _, cx| cx.notify()).detach();
        Self {
            nav,
            chat,
            skills,
            services,
            memory,
            settings,
        }
    }
}

impl RootView {
    /// The Android back button, bound to `escape` by `gpui-mobile`: it closes the open note before
    /// it leaves the app, like the Compose `BackHandler`.
    fn back(&mut self, cx: &mut Context<Self>) {
        if self.nav.read(cx).tab == Tab::Memory {
            self.memory.update(cx, |memory, cx| memory.back(cx));
        }
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.nav.read(cx).tab;
        // The Compose shell hides the bar while the IME is up, so the keyboard never covers it.
        let show_tabs = !keyboard_open();
        let tabs: Vec<AnyElement> = Tab::ALL
            .iter()
            .map(|tab| {
                let tab = *tab;
                let selected = tab == self.nav.read(cx).tab;
                let color = if selected {
                    rgb(0x1e6b52)
                } else {
                    rgb(0x637168)
                };
                div()
                    .id(format!("tab-{}", tab.label()))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .px_3()
                    .py_1()
                    .rounded_lg()
                    .when(selected, |this| this.bg(rgb(0xe1f0e8)))
                    .child(svg().data(tab.icon()).size(px(22.)).text_color(color))
                    .child(div().text_xs().text_color(color).child(tab.label()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.nav.update(cx, |nav, cx| {
                            nav.tab = tab;
                            cx.notify();
                        });
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();

        let body: AnyElement = match tab {
            Tab::Chat => self.chat.clone().into_any_element(),
            Tab::Skills => self.skills.clone().into_any_element(),
            Tab::Services => self.services.clone().into_any_element(),
            Tab::Memory => self.memory.clone().into_any_element(),
            Tab::Settings => self.settings.clone().into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xf7f9f6))
            .text_color(rgb(0x202c25))
            .on_action(cx.listener(|this, _: &Back, _, cx| this.back(cx)))
            .child(div().flex_1().min_h_0().child(body))
            .when(show_tabs, |this| {
                this.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_around()
                        .gap_1()
                        .px_2()
                        .py_2()
                        .bg(rgb(0xffffff))
                        .children(tabs),
                )
            })
    }
}

#[cfg(target_os = "android")]
fn keyboard_open() -> bool {
    gpui_mobile::keyboard_height() > 0.0
}

#[cfg(not(target_os = "android"))]
fn keyboard_open() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::Tab;

    #[test]
    fn tabs_are_the_five_kotlin_tabs_in_order() {
        assert_eq!(
            Tab::ALL.map(Tab::label),
            ["聊天", "技能", "服务", "记忆", "设置"]
        );
    }

    #[test]
    fn every_tab_has_its_icon() {
        for tab in Tab::ALL {
            assert!(!tab.icon().is_empty(), "{:?}", tab.label());
        }
    }
}
