//! `XiaomanRoot.kt` in GPUI: the five tabs and the screen each one shows.
//!
//! The Compose shell puts a `NavigationBar` at the bottom and swaps the body. Material icons have
//! no GPUI equivalent here, so each item is the tab's Chinese label in a pill; the selected tab
//! takes the primary container.
//!
//! The shell owns the [`Session`]: it starts the event pump that mirrors every streamed
//! [`AgentEvent`] into the chat screen, and refreshes the memory list when the agent writes a note.

use gpui::{
    ClickEvent, Context, Entity, IntoElement, Render, SharedString, Window, div, prelude::*, px,
};

use xiaoman_api::AgentEvent;
use xiaoman_app::session::{Session, SessionEvent};

use crate::theme::{Theme, theme};
use crate::view_model::chat::ChatModel;
use crate::view_model::copy;

use super::chat::{ChatEvent, ChatView};
use super::mcp::McpView;
use super::memory::MemoryView;
use super::settings::{SettingsSaved, SettingsView};
use super::skills::SkillsView;
use super::vocab::VocabView;

/// One tab of the bottom bar, in Compose order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Chat,
    Vocab,
    Skills,
    Services,
    Memory,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Chat,
        Tab::Vocab,
        Tab::Skills,
        Tab::Services,
        Tab::Memory,
        Tab::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => copy::NAV_CHAT,
            Self::Vocab => copy::NAV_VOCAB,
            Self::Skills => copy::NAV_SKILLS,
            Self::Services => copy::NAV_SERVICES,
            Self::Memory => copy::NAV_MEMORY,
            Self::Settings => copy::NAV_SETTINGS,
        }
    }
}

/// The root shell: one screen at a time, wired together through the session.
pub struct RootView {
    tab: Tab,
    session: Session,
    chat: Entity<ChatView>,
    vocab: Entity<VocabView>,
    skills: Entity<SkillsView>,
    mcp: Entity<McpView>,
    memory: Entity<MemoryView>,
    settings: Entity<SettingsView>,
}

impl RootView {
    pub fn new(cx: &mut Context<Self>, session: Session) -> Self {
        let chat = cx.new(|cx| {
            let mut view = ChatView::new(cx, ChatModel::new(), session.clone());
            // An empty transcript still carries 待设置 / 已就绪 from the saved settings.
            view.sync(cx);
            view
        });
        let vocab = cx.new(|cx| VocabView::new(cx, session.clone()));
        let skills = cx.new(|cx| SkillsView::new(cx, session.clone()));
        let mcp = cx.new(|cx| McpView::new(cx, session.clone()));
        let memory = cx.new(|cx| MemoryView::new(cx, session.clone()));
        let settings = cx.new(|cx| SettingsView::new(cx, session.clone()));

        // A saved settings form decides whether the chat header still says 待设置.
        cx.subscribe(&settings, |this, _settings, event: &SettingsSaved, cx| {
            this.chat.update(cx, |chat, cx| {
                chat.set_configured(event.configured);
                cx.notify();
            });
        })
        .detach();

        // The chat banner's 去设置 action.
        cx.subscribe(&chat, |this, _chat, event: &ChatEvent, cx| {
            if *event == ChatEvent::OpenSettings {
                this.tab = Tab::Settings;
                cx.notify();
            }
        })
        .detach();

        // The agent's events: the chat transcript follows every one, the memory list follows a
        // written note; a state change (a turn ending, a prompt answered) refreshes the screen too.
        let mut events = session.subscribe();
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                let applied = this.update(cx, |this, cx| match event {
                    SessionEvent::Agent(event) => this.on_event(&event, cx),
                    SessionEvent::State => {
                        this.chat.update(cx, |chat, cx| chat.sync(cx));
                        cx.notify();
                    }
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            tab: Tab::Chat,
            session,
            chat,
            vocab,
            skills,
            mcp,
            memory,
            settings,
        }
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn chat(&self) -> &Entity<ChatView> {
        &self.chat
    }

    pub fn settings(&self) -> &Entity<SettingsView> {
        &self.settings
    }

    fn on_event(&mut self, event: &AgentEvent, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cx| chat.sync(cx));
        if matches!(event, AgentEvent::MemoryUpdated { .. }) {
            self.memory.update(cx, |memory, cx| memory.reload(cx));
        }
        cx.notify();
    }

    fn nav_item(&self, theme: &Theme, tab: Tab, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.tab == tab;
        let on_click = cx.listener(move |this, _event: &ClickEvent, _window, cx| {
            this.tab = tab;
            if tab == Tab::Memory {
                this.memory.update(cx, |memory, cx| memory.reload(cx));
            }
            if tab == Tab::Skills {
                // A skill saved from the chat must be visible without a manual reload.
                this.skills.update(cx, |skills, cx| skills.reload(cx));
            }
            if tab == Tab::Vocab {
                // The chat tools may have created the day's batch or graded words.
                this.vocab.update(cx, |vocab, cx| vocab.reload(cx));
            }
            cx.notify();
        });
        div()
            .id(SharedString::from(format!("tab-{}", tab.label())))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .flex_1()
            .py_2()
            .gap_1()
            .cursor_pointer()
            .child(div().w(px(28.)).h(px(3.)).rounded_full().bg(if selected {
                theme.primary
            } else {
                theme.background
            }))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .bg(if selected {
                        theme.primary_container
                    } else {
                        theme.surface
                    })
                    .text_size(Theme::small_size())
                    .text_color(if selected {
                        theme.on_primary_container
                    } else {
                        theme.on_surface_variant
                    })
                    .child(tab.label()),
            )
            .on_click(on_click)
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let body = match self.tab {
            Tab::Chat => self.chat.clone().into_any_element(),
            Tab::Vocab => self.vocab.clone().into_any_element(),
            Tab::Skills => self.skills.clone().into_any_element(),
            Tab::Services => self.mcp.clone().into_any_element(),
            Tab::Memory => self.memory.clone().into_any_element(),
            Tab::Settings => self.settings.clone().into_any_element(),
        };

        let mut nav = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .w_full()
            .bg(theme.surface)
            .pb_2()
            .border_t_1();
        nav = nav.border_color(theme.outline_variant);
        for tab in Tab::ALL {
            nav = nav.child(self.nav_item(&theme, tab, cx));
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(body),
            )
            .child(nav)
    }
}
