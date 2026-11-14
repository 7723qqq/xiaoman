//! The six-tab shell, mirroring the desktop `xiaoman-ui` shell.

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
use crate::vocab::VocabView;

gpui::actions!(xiaoman, [Back]);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    #[default]
    Chat,
    Vocab,
    Skills,
    Services,
    Memory,
    Settings,
}

impl Tab {
    const ALL: [Tab; 6] = [
        Tab::Chat,
        Tab::Vocab,
        Tab::Skills,
        Tab::Services,
        Tab::Memory,
        Tab::Settings,
    ];

    /// The bar icon, one outlined glyph per tab.
    fn icon(self) -> &'static [u8] {
        match self {
            Self::Chat => include_bytes!("assets/icons/chat.svg"),
            Self::Vocab => include_bytes!("assets/icons/vocab.svg"),
            Self::Skills => include_bytes!("assets/icons/skills.svg"),
            Self::Services => include_bytes!("assets/icons/services.svg"),
            Self::Memory => include_bytes!("assets/icons/memory.svg"),
            Self::Settings => include_bytes!("assets/icons/settings.svg"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Chat => "聊天",
            Self::Vocab => "背单词",
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
    vocab: Entity<VocabView>,
    skills: Entity<SkillsView>,
    services: Entity<ServicesView>,
    memory: Entity<MemoryView>,
    settings: Entity<SettingsView>,
}

impl RootView {
    pub fn new(session: Option<Arc<Session>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|_| Nav::default());
        let chat = cx.new(|cx| ChatView::new(session.clone(), nav.clone(), window, cx));
        let vocab = cx.new(|cx| VocabView::new(session.clone(), nav.clone(), cx));
        let skills = cx.new(|cx| SkillsView::new(session.clone(), cx));
        let services = cx.new(|cx| ServicesView::new(session.clone(), window, cx));
        let memory = cx.new(|cx| MemoryView::new(session.clone(), window, cx));
        let settings = cx.new(|cx| SettingsView::new(session.clone(), window, cx));
        // A banner may switch the tab from outside this view.
        cx.observe(&nav, |_, _, cx| cx.notify()).detach();
        Self {
            nav,
            chat,
            vocab,
            skills,
            services,
            memory,
            settings,
        }
    }
}

impl RootView {
    /// The Android back button, bound to `escape` by `gpui-mobile`: while the keyboard is open it
    /// only closes the keyboard; otherwise it closes the open note before it leaves the app, like
    /// the Android back handler.
    fn back(&mut self, cx: &mut Context<Self>) {
        if keyboard_open() {
            gpui_mobile::hide_keyboard();
            return;
        }
        let on_memory = self.nav.read(cx).tab == Tab::Memory;
        if on_memory && self.memory.update(cx, |memory, cx| memory.back(cx)) {
            return;
        }
        move_task_to_back();
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.nav.read(cx).tab;
        // Hide the bar while the IME is up, so the keyboard never covers it.
        let show_tabs = !keyboard_open();
        // The IME no longer resizes the window (targetSdk 35 edge-to-edge), so pad the content
        // up by the keyboard height — the equivalent of Compose's `imePadding()`.
        let keyboard = keyboard_height();
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
                    .on_click(cx.listener(move |this, _, window, cx| {
                        // A focused input on the outgoing page keeps its GPUI focus handle after
                        // its element is removed; without this, switching back re-fires Focus and
                        // re-opens the IME without a tap.
                        window.blur(cx);
                        this.nav.update(cx, |nav, cx| {
                            nav.tab = tab;
                            cx.notify();
                        });
                        if tab == crate::app::Tab::Skills {
                            // A skill saved from the chat must be visible without a manual reload.
                            this.skills.update(cx, |view, cx| view.reload(cx));
                        }
                        if tab == crate::app::Tab::Vocab {
                            // The chat tools may have created the day's batch or graded words.
                            this.vocab.update(cx, |view, cx| view.reload(cx));
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();

        let body: AnyElement = match tab {
            Tab::Chat => self.chat.clone().into_any_element(),
            Tab::Vocab => self.vocab.clone().into_any_element(),
            Tab::Skills => self.skills.clone().into_any_element(),
            Tab::Services => self.services.clone().into_any_element(),
            Tab::Memory => self.memory.clone().into_any_element(),
            Tab::Settings => self.settings.clone().into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .pb(px(keyboard))
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
fn keyboard_height() -> f32 {
    gpui_mobile::keyboard_height()
}

#[cfg(not(target_os = "android"))]
fn keyboard_height() -> f32 {
    0.0
}

#[cfg(target_os = "android")]
fn keyboard_open() -> bool {
    keyboard_height() > 0.0
}

#[cfg(not(target_os = "android"))]
fn keyboard_open() -> bool {
    false
}

#[cfg(target_os = "android")]
fn move_task_to_back() {
    gpui_mobile::android::jni::move_task_to_back();
}

#[cfg(not(target_os = "android"))]
fn move_task_to_back() {}

#[cfg(test)]
mod tests {
    use super::Tab;

    #[test]
    fn tabs_are_the_six_screens_in_order() {
        assert_eq!(
            Tab::ALL.map(Tab::label),
            ["聊天", "背单词", "技能", "服务", "记忆", "设置"]
        );
    }

    #[test]
    fn every_tab_has_its_icon() {
        for tab in Tab::ALL {
            assert!(!tab.icon().is_empty(), "{:?}", tab.label());
        }
    }
}
