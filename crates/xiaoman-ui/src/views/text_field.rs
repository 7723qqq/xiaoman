//! A single-line text field, because GPUI 0.2 ships no text input widget.
//!
//! On Windows every typed character and every IME composition arrives only at a registered
//! [`InputHandler`] (`WM_CHAR` / `WM_IME_COMPOSITION`), so the field is an entity implementing
//! [`EntityInputHandler`] and its element registers `ElementInputHandler` during paint. Named keys
//! (Backspace, arrows, Home, End, Delete, Enter) never arrive as text, so they are handled by
//! `on_key_down` instead — no character is ever inserted twice.
//!
//! Deliberate simplifications: no selection, no mouse-positioned caret, and only the paste
//! shortcut (Ctrl+V) from the clipboard. The caret is drawn where the byte offset points, and the
//! IME candidate window is anchored to the field's left edge.

use std::ops::Range;

use gpui::{
    App, Bounds, Context, Div, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, GlobalElementId, HighlightStyle, IntoElement,
    KeyDownEvent, LayoutId, MouseButton, Pixels, Point, Render, SharedString, Size, StyledText,
    TextStyle, UTF16Selection, Window, div, prelude::*, px,
};

use crate::theme::{Theme, theme};

/// Emitted after every edit, so a parent view can mirror the text into its view model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChanged {
    pub text: SharedString,
}

/// Emitted when the user presses Enter inside the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSubmitted {
    pub text: SharedString,
}

/// A one-line text field with a caret and an IME-aware input handler.
pub struct TextField {
    text: String,
    /// Byte offset of the caret, always on a char boundary.
    caret: usize,
    /// The IME's composing range in UTF-16 units, while a composition is in progress.
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    masked: bool,
    focus: FocusHandle,
}

impl EventEmitter<FieldChanged> for TextField {}
impl EventEmitter<FieldSubmitted> for TextField {}

impl TextField {
    pub fn new(
        cx: &mut Context<Self>,
        placeholder: impl Into<SharedString>,
        initial: &str,
    ) -> Self {
        Self {
            text: initial.to_string(),
            caret: initial.len(),
            marked: None,
            placeholder: placeholder.into(),
            masked: false,
            focus: cx.focus_handle(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        self.caret = self.text.len();
        self.marked = None;
        cx.emit(FieldChanged {
            text: self.text.clone().into(),
        });
        cx.notify();
    }

    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        if self.masked != masked {
            self.masked = masked;
            cx.notify();
        }
    }

    /// Keeps the hint in step with the session state (answer a question vs. start a turn).
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            cx.notify();
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.text.clear();
        self.caret = 0;
        self.marked = None;
        cx.emit(FieldChanged {
            text: SharedString::default(),
        });
        cx.notify();
    }

    pub fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus
    }

    /// What the field paints: the real text, or one dot per character for a secret.
    fn display(&self, part: &str) -> String {
        if self.masked {
            "•".repeat(part.chars().count())
        } else {
            part.to_string()
        }
    }

    fn insert_str(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.replace(['\n', '\r'], " ");
        self.text.insert_str(self.caret, &text);
        self.caret += text.len();
        self.marked = None;
        cx.emit(FieldChanged {
            text: self.text.clone().into(),
        });
        cx.notify();
    }

    fn replace_utf16(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let start = self.byte_of_utf16(range.start);
        let end = self.byte_of_utf16(range.end);
        self.text.replace_range(start..end, text);
        self.caret = start + text.len();
        cx.emit(FieldChanged {
            text: self.text.clone().into(),
        });
        cx.notify();
    }

    fn byte_of_utf16(&self, offset: usize) -> usize {
        let mut units = 0;
        for (byte, character) in self.text.char_indices() {
            if units >= offset {
                return byte;
            }
            units += character.len_utf16();
        }
        self.text.len()
    }

    fn utf16_of_byte(&self, byte: usize) -> usize {
        self.text[..byte.min(self.text.len())]
            .encode_utf16()
            .count()
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        let Some((start, _)) = self.text[..self.caret].char_indices().next_back() else {
            return;
        };
        self.text.replace_range(start..self.caret, "");
        self.caret = start;
        cx.emit(FieldChanged {
            text: self.text.clone().into(),
        });
        cx.notify();
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        let Some((_, character)) = self.text[self.caret..].char_indices().next() else {
            return;
        };
        let end = self.caret + character.len_utf8();
        self.text.replace_range(self.caret..end, "");
        cx.emit(FieldChanged {
            text: self.text.clone().into(),
        });
        cx.notify();
    }

    fn move_left(&mut self, cx: &mut Context<Self>) {
        if let Some((start, _)) = self.text[..self.caret].char_indices().next_back() {
            self.caret = start;
            cx.notify();
        }
    }

    fn move_right(&mut self, cx: &mut Context<Self>) {
        if let Some((_, character)) = self.text[self.caret..].char_indices().next() {
            self.caret += character.len_utf8();
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.control || event.keystroke.modifiers.platform {
            // Ctrl+V is the only clipboard shortcut a single-line field needs: an API key arrives
            // by paste far more often than by typing.
            if event.keystroke.key == "v"
                && let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
            {
                self.insert_str(&text, cx);
            }
            return;
        }
        match event.keystroke.key.as_str() {
            "backspace" => self.backspace(cx),
            "delete" => self.delete_forward(cx),
            "left" => self.move_left(cx),
            "right" => self.move_right(cx),
            "home" => {
                self.caret = 0;
                cx.notify();
            }
            "end" => {
                self.caret = self.text.len();
                cx.notify();
            }
            "enter" => {
                let text: SharedString = self.text.clone().into();
                cx.emit(FieldSubmitted { text });
                window.blur();
            }
            _ => {}
        }
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let start = self.byte_of_utf16(range_utf16.start);
        let end = self.byte_of_utf16(range_utf16.end);
        *adjusted_range = Some(range_utf16);
        Some(self.text[start..end].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let caret = self.utf16_of_byte(self.caret);
        Some(UTF16Selection {
            range: caret..caret,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked.clone()
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = replacement_range
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| {
                let caret = self.utf16_of_byte(self.caret);
                caret..caret
            });
        self.replace_utf16(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| {
                let caret = self.utf16_of_byte(self.caret);
                caret..caret
            });
        let start = self.byte_of_utf16(range.start);
        self.replace_utf16(range, new_text, cx);
        let start = self.utf16_of_byte(start);
        self.marked = Some(start..start + new_text.encode_utf16().count());
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // The candidate window sits at the field's left edge, on the text baseline.
        Some(Bounds {
            origin: element_bounds.origin,
            size: Size {
                width: px(1.),
                height: element_bounds.size.height,
            },
        })
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for TextField {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let focused = self.focus.is_focused(_window);
        let before = self.text[..self.caret].to_string();
        let after = self.text[self.caret..].to_string();
        let empty = self.text.is_empty();

        let mut field = div()
            .flex()
            .items_center()
            .w_full()
            .h(px(40.))
            .px_3()
            .rounded_lg()
            .border_1()
            .border_color(if focused {
                theme.primary
            } else {
                theme.outline_variant
            })
            .bg(theme.surface)
            .overflow_hidden()
            .cursor_text()
            .track_focus(&self.focus);

        if empty {
            field = field.child(
                div()
                    .whitespace_nowrap()
                    .text_size(Theme::body_size())
                    .text_color(theme.on_surface_variant)
                    .child(self.placeholder.clone()),
            );
        } else {
            field = field.child(
                div()
                    .whitespace_nowrap()
                    .text_size(Theme::body_size())
                    .text_color(theme.on_surface)
                    .child(self.display(&before)),
            );
        }

        if focused {
            field = field.child(
                div()
                    .w(px(1.5))
                    .h(px(18.))
                    .flex_shrink_0()
                    .bg(theme.primary),
            );
        }

        if !after.is_empty() {
            field = field.child(
                div()
                    .whitespace_nowrap()
                    .text_size(Theme::body_size())
                    .text_color(theme.on_surface)
                    .child(self.display(&after)),
            );
        }
        let focus = self.focus.clone();
        let field = field
            .on_mouse_down(MouseButton::Left, move |_event, window, _cx| {
                focus.focus(window);
            })
            .on_key_down(cx.listener(Self::on_key_down));

        InputElement {
            child: field,
            focus: self.focus.clone(),
            view: cx.entity(),
        }
    }
}

/// Paints the field and, in the paint phase, registers the IME/text input handler for it.
///
/// `Window::handle_input` may only be called while painting, which is why this wrapper exists:
/// a plain `div` has no such hook.
struct InputElement {
    child: Div,
    focus: FocusHandle,
    view: Entity<TextField>,
}

impl IntoElement for InputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for InputElement {
    type RequestLayoutState = <Div as Element>::RequestLayoutState;
    type PrepaintState = <Div as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Element::id(&self.child)
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        Element::request_layout(&mut self.child, id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        Element::prepaint(
            &mut self.child,
            id,
            inspector_id,
            bounds,
            request_layout,
            window,
            cx,
        )
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        Element::paint(
            &mut self.child,
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        );
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
    }
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
