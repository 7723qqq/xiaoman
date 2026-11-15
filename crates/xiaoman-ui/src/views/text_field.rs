//! Text input: a thin shim over `gpui-kit`'s input widget.
//!
//! Both front-ends run the same GPUI line (`gpui-pre` 0.3), so the desktop uses the very
//! `InputState` + `Input` the Android app uses. Selection, Ctrl+A / copy / cut / paste, drag
//! select, the right-click edit menu, IME composition and word-wise navigation all come from the
//! component library instead of a hand-rolled field.

use std::ops::Range;

use gpui::{
    App, AppContext as _, Context, Entity, HighlightStyle, SharedString, StyledText, TextStyle,
    Window,
};
use gpui_kit::component::input::InputState;

use crate::theme::Theme;

pub use gpui_kit::component::input::{Input, InputEvent};

/// The desktop screens talk about a "text field"; it is the component library's input state.
pub type TextField = InputState;

/// One single-line input, optionally pre-filled.
pub fn text_field<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    placeholder: impl Into<SharedString>,
    initial: &str,
) -> Entity<InputState> {
    build(window, cx, placeholder, initial, false)
}

/// One single-line input whose value is masked (an API key), with the eye toggle.
pub fn text_field_masked<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    placeholder: impl Into<SharedString>,
    initial: &str,
) -> Entity<InputState> {
    build(window, cx, placeholder, initial, true)
}

fn build<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    placeholder: impl Into<SharedString>,
    initial: &str,
    masked: bool,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    let initial = SharedString::from(initial.to_string());
    cx.new(|cx| {
        let mut state = InputState::new(window, cx).placeholder(placeholder);
        if masked {
            state = state.masked(true);
        }
        if !initial.is_empty() {
            state.set_value(initial.clone(), window, cx);
        }
        state
    })
}

/// The element for a plain field: the library input at the app's control height.
pub fn input(state: &Entity<InputState>) -> Input {
    Input::new(state).h(Theme::control_height())
}

/// The element for a masked field, with the library's eye toggle.
pub fn masked_input(state: &Entity<InputState>) -> Input {
    Input::new(state).h(Theme::control_height()).mask_toggle()
}

/// The field's text, without the surrounding whitespace a form would trim.
pub fn field_value(field: &Entity<InputState>, cx: &App) -> String {
    field.read(cx).value().to_string()
}

/// Replaces the field's whole value (programmatic edits only; typing edits itself).
pub fn set_field_value(
    field: &Entity<InputState>,
    value: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) {
    let value = value.into();
    field.update(cx, |state, cx| state.set_value(value, window, cx));
}

/// A highlighted run of text, for the chat bubbles that carry `**bold**`.
pub fn styled_text(text: &str, theme: &Theme, spans: &[(Range<usize>, bool)]) -> StyledText {
    let style = TextStyle {
        color: theme.on_surface,
        font_size: Theme::body_size().into(),
        line_height: Theme::body_line_height().into(),
        ..TextStyle::default()
    };
    let highlights = spans.iter().map(|(range, bold)| {
        (
            range.clone(),
            HighlightStyle {
                font_weight: bold.then_some(Theme::SEMIBOLD),
                ..HighlightStyle::default()
            },
        )
    });
    StyledText::new(text.to_string()).with_default_highlights(&style, highlights)
}
