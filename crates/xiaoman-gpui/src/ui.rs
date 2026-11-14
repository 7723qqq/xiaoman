//! Small shared helpers for the GPUI screens.

use gpui::{AppContext as _, Context, Entity, ParentElement, SharedString, Styled, Window};
#[cfg(target_os = "android")]
use gpui_kit::component::input::InputEvent;
use gpui_kit::component::input::InputState;

/// One single-line input, optionally pre-filled.
pub fn input<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    placeholder: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> Entity<InputState> {
    input_with_keyboard_type(
        window,
        cx,
        gpui_mobile::KeyboardType::Default,
        placeholder,
        value,
    )
}

/// One single-line input with a chosen keyboard type (URL / number / …).
pub fn input_with_keyboard_type<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    keyboard_type: gpui_mobile::KeyboardType,
    placeholder: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    let value = value.into();
    let entity = cx.new(|cx| {
        let mut state = InputState::new(window, cx).placeholder(placeholder);
        if !value.is_empty() {
            state.set_value(value, window, cx);
        }
        state
    });
    // The form fields use the default IME action; the chat input asks for Send.
    #[cfg(target_os = "android")]
    {
        cx.subscribe_in(&entity, window, move |_, _, event, _, _| {
            if matches!(event, InputEvent::Focus) {
                gpui_mobile::show_keyboard_with_type(keyboard_type);
            }
        })
        .detach();
    }
    #[cfg(not(target_os = "android"))]
    let _ = keyboard_type;
    entity
}

/// A single-line input that masks its value by default (an API key).
pub fn input_masked<T: 'static>(
    window: &mut Window,
    cx: &mut Context<T>,
    placeholder: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    let value = value.into();
    cx.new(|cx| {
        let mut state = InputState::new(window, cx)
            .placeholder(placeholder)
            .masked(true);
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

/// A labelled form field whose value is masked, with an eye toggle to reveal it.
pub fn field_masked(label: &str, input: &Entity<InputState>) -> impl gpui::IntoElement {
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
        .child(gpui_kit::component::input::Input::new(input).mask_toggle())
}
