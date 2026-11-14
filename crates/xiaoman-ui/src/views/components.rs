//! The Material-ish pieces the screens use: a surface card, a banner, filled
//! and outlined buttons, a switch, a chip and a divider.
//!
//! GPUI 0.2 has no widget library, so these are plain `div`s. They take the click handler already
//! bound to the owning view (`cx.listener`), which keeps every screen free of `Element` plumbing.

use gpui::{
    App, ClickEvent, Div, FontWeight, Hsla, IntoElement, Pixels, SharedString, Styled, Window, div,
    prelude::*, px,
};

use crate::theme::Theme;

/// A rounded surface with the Compose card border.
pub fn card(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .rounded_lg()
        .border_1()
        .border_color(theme.outline_variant)
        .bg(theme.surface)
}

/// A plain column with no chrome.
pub fn column() -> Div {
    div().flex().flex_col().w_full()
}

/// A horizontal row, vertically centered.
pub fn row() -> Div {
    div().flex().flex_row().items_center().w_full()
}

pub fn divider(theme: &Theme) -> Div {
    div().w_full().h(px(1.)).bg(theme.outline_variant)
}

/// `MaterialTheme.typography.titleLarge`-ish heading.
pub fn heading(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(22.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.on_surface)
        .child(text.into())
}

/// `titleMedium`: 18sp semibold.
pub fn title(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(Theme::title_size())
        .font_weight(Theme::SEMIBOLD)
        .text_color(theme.on_surface)
        .child(text.into())
}

/// `titleSmall`: 15sp semibold.
pub fn title_small(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(Theme::title_small_size())
        .font_weight(Theme::SEMIBOLD)
        .text_color(theme.on_surface)
        .child(text.into())
}

/// `bodyMedium`.
pub fn body(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(Theme::body_size())
        .line_height(Theme::body_line_height())
        .text_color(theme.on_surface)
        .child(text.into())
}

/// `bodySmall`, for supporting lines.
pub fn small(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(Theme::small_size())
        .line_height(Theme::small_line_height())
        .text_color(theme.on_surface_variant)
        .child(text.into())
}

/// `labelSmall`, for badges and captions.
pub fn label(_theme: &Theme, text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .text_size(Theme::label_size())
        .line_height(Theme::label_line_height())
        .text_color(color)
        .child(text.into())
}

/// A pill: the header status chip, the official/community badge.
pub fn pill(_theme: &Theme, text: impl Into<SharedString>, fg: Hsla, bg: Hsla) -> Div {
    div()
        .flex()
        .items_center()
        .px_3()
        .py_1()
        .rounded_full()
        .bg(bg)
        .text_size(Theme::label_size())
        .text_color(fg)
        .child(text.into())
}

/// The chat header's banner row: text on the left, an action on the right.
pub fn banner(
    theme: &Theme,
    text: impl Into<SharedString>,
    action: impl Into<SharedString>,
    on_action: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let theme = *theme;
    row()
        .w_full()
        .px_4()
        .py_1()
        .gap_2()
        .bg(theme.secondary_container)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(Theme::small_size())
                .text_color(theme.on_secondary_container)
                .child(text.into()),
        )
        .child(
            div()
                .id("banner-action")
                .flex_shrink_0()
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_size(Theme::label_size())
                .text_color(theme.primary)
                .child(action.into())
                .on_click(on_action),
        )
}

/// A filled button (Material `Button`).
pub fn filled_button(
    theme: &Theme,
    text: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let theme = *theme;
    let text: SharedString = text.into();
    let (background, color) = if enabled {
        (theme.primary, theme.on_primary)
    } else {
        (theme.surface_variant, theme.on_surface_variant)
    };
    div()
        .id(SharedString::from(format!("filled-{}", text.clone())))
        .flex()
        .items_center()
        .justify_center()
        .px_4()
        .py_2()
        .rounded_lg()
        .bg(background)
        .text_size(Theme::body_size())
        .text_color(color)
        .when(enabled, |element| element.cursor_pointer())
        .child(text)
        .when(enabled, |element| element.on_click(on_click))
}

/// An outlined button (Material `OutlinedButton`).
pub fn outlined_button(
    theme: &Theme,
    text: impl Into<SharedString>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let theme = *theme;
    let text: SharedString = text.into();
    div()
        .id(SharedString::from(format!("outlined-{}", text.clone())))
        .flex()
        .items_center()
        .justify_center()
        .px_4()
        .py_2()
        .rounded_lg()
        .border_1()
        .border_color(if enabled {
            theme.outline_variant
        } else {
            theme.surface_variant
        })
        .text_size(Theme::body_size())
        .text_color(if enabled {
            theme.on_surface
        } else {
            theme.on_surface_variant
        })
        .when(enabled, |element| element.cursor_pointer())
        .child(text)
        .when(enabled, |element| element.on_click(on_click))
}

/// A text button: no chrome until hovered.
///
/// The element id is derived from the label, so two screens that paint the same label share it —
/// pass an explicit id with [`text_button_with_id`] when the same label appears twice in a window.
pub fn text_button(
    theme: &Theme,
    text: impl Into<SharedString>,
    color: Hsla,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let text: SharedString = text.into();
    text_button_with_id(
        SharedString::from(format!("text-{text}")),
        theme,
        text,
        color,
        enabled,
        on_click,
    )
}

/// A text button with an id of the caller's choosing.
pub fn text_button_with_id(
    id: impl Into<SharedString>,
    theme: &Theme,
    text: impl Into<SharedString>,
    color: Hsla,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .px_2()
        .py_1()
        .rounded_md()
        .text_size(Theme::small_size())
        .text_color(if enabled {
            color
        } else {
            theme.on_surface_variant
        })
        .when(enabled, |element| element.cursor_pointer())
        .child(text.into())
        .when(enabled, |element| element.on_click(on_click))
}

/// A Material `Switch`, drawn as a track with a knob.
pub fn toggle(
    theme: &Theme,
    on: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let theme = *theme;
    let track = if on {
        theme.primary
    } else {
        theme.outline_variant
    };
    let knob = if on { theme.on_primary } else { theme.surface };
    div()
        .id("switch")
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .w(px(44.))
        .h(px(24.))
        .px(px(3.))
        .rounded_full()
        .bg(track)
        .cursor_pointer()
        .when(on, |element| element.justify_end())
        .when(!on, |element| element.justify_start())
        .child(div().w(px(18.)).h(px(18.)).rounded_full().bg(knob))
        .on_click(on_click)
}

/// A Material `FilterChip`.
pub fn chip(
    theme: &Theme,
    text: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let theme = *theme;
    let text: SharedString = text.into();
    div()
        .id(SharedString::from(format!("chip-{}", text.clone())))
        .flex()
        .items_center()
        .px_3()
        .py_1()
        .rounded_lg()
        .border_1()
        .border_color(if selected {
            theme.primary
        } else {
            theme.outline_variant
        })
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
        .cursor_pointer()
        .child(text)
        .on_click(on_click)
}

/// A labelled text input row: `label` above the field the caller built.
pub fn field_block(theme: &Theme, text: impl Into<SharedString>, field: impl IntoElement) -> Div {
    column()
        .gap_1()
        .py_1()
        .child(label(theme, text, theme.on_surface_variant))
        .child(field)
}

/// The error-colored supporting text under an invalid field.
pub fn field_error(theme: &Theme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(Theme::small_size())
        .text_color(theme.error)
        .child(text.into())
}

/// A fixed-size square, used for the avatar and the tab icons.
pub fn glyph_box(
    _theme: &Theme,
    glyph: impl Into<SharedString>,
    size: Pixels,
    fg: Hsla,
    bg: Hsla,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .w(size)
        .h(size)
        .rounded_full()
        .bg(bg)
        .text_size(px(15.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(glyph.into())
}

/// Empty space of a given height, for the gaps Compose gets from `Arrangement.spacedBy`.
pub fn gap(height: Pixels) -> Div {
    div().h(height).w(px(1.))
}
