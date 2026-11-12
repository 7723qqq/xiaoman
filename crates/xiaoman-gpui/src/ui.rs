//! Small shared helpers for the GPUI screens.

use gpui::{AppContext as _, Context, Entity, ParentElement, SharedString, Styled, Window};
use gpui_kit::component::input::InputState;

/// One single-line input, optionally pre-filled.
pub fn input<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    placeholder: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    let value = value.into();
    cx.new(|cx| {
        let mut state = InputState::new(window, cx).placeholder(placeholder);
        if !value.is_empty() {
            state.set_value(value, window, cx);
        }
        state
    })
}

/// The value of an input, trimmed.
pub fn value<T>(input: &Entity<InputState>, cx: &Context<T>) -> String {
    input.read(cx).value().trim().to_string()
}

/// A section title.
pub fn title(text: &str) -> impl gpui::IntoElement {
    gpui::div().text_lg().child(text.to_string())
}

/// A secondary line.
pub fn hint(text: impl Into<String>) -> impl gpui::IntoElement {
    gpui::div()
        .text_sm()
        .text_color(gpui::rgb(0x637168))
        .child(text.into())
}

/// A labelled form field.
pub fn field(label: &str, input: &Entity<InputState>) -> impl gpui::IntoElement {
    gpui::div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            gpui::div()
                .text_sm()
                .text_color(gpui::rgb(0x637168))
                .child(label.to_string()),
        )
        .child(gpui_kit::component::input::Input::new(input))
}
