//! A single-line text field, because GPUI 0.2 ships no text input widget.
//!
//! On Windows every typed character and every IME composition arrives only at a registered
//! [`InputHandler`] (`WM_CHAR` / `WM_IME_COMPOSITION`), so the field is an entity implementing
//! [`EntityInputHandler`] and its element registers `ElementInputHandler` during paint. Named keys
//! (Backspace, arrows, Home, End, Delete, Enter) never arrive as text, so they are handled by
//! `on_key_down` instead — no character is ever inserted twice.
//!
//! Selection is supported: Ctrl+A selects everything, Ctrl+C / Ctrl+X copy or cut it, Shift with
//! the arrow / Home / End keys extends it, and the mouse sets the caret on press and extends the
//! selection while dragging. Typing, Backspace and Delete replace the selection. The caret and the
//! selected run are painted as ordinary divs, so no text shaping is needed to draw them; the mouse
//! hit test shapes one line to find the index under the pointer.
//!
//! Known simplifications: the IME candidate window is anchored to the field's left edge, and the
//! field does not scroll when the text is wider than the box.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, Div, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, HighlightStyle,
    IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Render, SharedString, Size, StyledText, TextRun, TextStyle, UTF16Selection,
    Window, div, font, point, prelude::*, px,
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

/// The editable state: text plus a caret and a selection anchor, both byte offsets.
///
/// Pure logic, so `tests/text_field.rs` can pin the selection rules without a window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Buffer {
    text: String,
    caret: usize,
    /// Where the selection started; equal to [`Buffer::caret`] when nothing is selected.
    anchor: usize,
}

impl Buffer {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let caret = text.len();
        Self {
            text,
            caret,
            anchor: caret,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selected byte range, `None` when the selection is empty.
    pub fn selection(&self) -> Option<Range<usize>> {
        let start = self.anchor.min(self.caret);
        let end = self.anchor.max(self.caret);
        (start != end).then_some(start..end)
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|range| &self.text[range])
    }

    /// Replaces everything and puts the caret at the end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.caret = self.text.len();
        self.anchor = self.caret;
    }

    pub fn clear(&mut self) {
        self.set_text("");
    }

    /// The nearest char boundary at or before `index`, so a caret never splits a character.
    pub fn floor_boundary(&self, index: usize) -> usize {
        let len = self.text.len();
        if index >= len {
            return len;
        }
        let mut floor = 0;
        for (offset, _) in self.text.char_indices() {
            if offset > index {
                break;
            }
            floor = offset;
        }
        floor
    }

    /// Inserts `text` where the selection is (replacing it). Returns whether anything changed.
    pub fn insert(&mut self, text: &str) -> bool {
        let text = text.replace(['\n', '\r'], " ");
        match self.selection() {
            Some(range) => {
                self.text.replace_range(range.clone(), &text);
                self.caret = range.start + text.len();
            }
            None => {
                self.text.insert_str(self.caret, &text);
                self.caret += text.len();
            }
        }
        self.anchor = self.caret;
        true
    }

    /// Deletes the selection, or the character before the caret. Returns whether it changed.
    pub fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let Some((start, _)) = self.text[..self.caret].char_indices().next_back() else {
            return false;
        };
        self.text.replace_range(start..self.caret, "");
        self.caret = start;
        self.anchor = start;
        true
    }

    /// Deletes the selection, or the character after the caret. Returns whether it changed.
    pub fn delete_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        let Some((_, character)) = self.text[self.caret..].char_indices().next() else {
            return false;
        };
        let end = self.caret + character.len_utf8();
        self.text.replace_range(self.caret..end, "");
        true
    }

    fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection() else {
            return false;
        };
        self.text.replace_range(range.clone(), "");
        self.caret = range.start;
        self.anchor = range.start;
        true
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    /// Puts the caret at `index`; `extend` (Shift) keeps the anchor so a selection grows.
    pub fn move_to(&mut self, index: usize, extend: bool) {
        self.caret = self.floor_boundary(index);
        if !extend {
            self.anchor = self.caret;
        }
    }

    pub fn move_left(&mut self, extend: bool) {
        if !extend && let Some(range) = self.selection() {
            self.move_to(range.start, false);
            return;
        }
        let previous = self.text[..self.caret]
            .char_indices()
            .next_back()
            .map(|(offset, _)| offset)
            .unwrap_or(0);
        self.move_to(previous, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        if !extend && let Some(range) = self.selection() {
            self.move_to(range.end, false);
            return;
        }
        let next = self.text[self.caret..]
            .char_indices()
            .next()
            .map(|(_, character)| self.caret + character.len_utf8())
            .unwrap_or(self.text.len());
        self.move_to(next, extend);
    }

    pub fn move_home(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn move_end(&mut self, extend: bool) {
        let end = self.text.len();
        self.move_to(end, extend);
    }
}

/// A one-line text field with a caret, a selection and an IME-aware input handler.
pub struct TextField {
    buffer: Buffer,
    /// The IME's composing range in UTF-16 units, while a composition is in progress.
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    masked: bool,
    focus: FocusHandle,
    /// Where the text starts, in window coordinates; used by the mouse hit test.
    text_origin: Point<Pixels>,
    /// A drag is in progress: mouse moves extend the selection instead of moving the caret.
    dragging: bool,
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
            buffer: Buffer::new(initial),
            marked: None,
            placeholder: placeholder.into(),
            masked: false,
            focus: cx.focus_handle(),
            text_origin: Point::default(),
            dragging: false,
        }
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    pub fn masked(mut self, masked: bool) -> Self {
        self.masked = masked;
        self
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.buffer.set_text(text);
        self.marked = None;
        self.changed(cx);
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
        self.buffer.clear();
        self.marked = None;
        self.changed(cx);
    }

    pub fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(FieldChanged {
            text: self.buffer.text().to_string().into(),
        });
        cx.notify();
    }

    /// What one part of the field paints: the real text, or one dot per character for a secret.
    fn display(&self, part: &str) -> String {
        if self.masked {
            "•".repeat(part.chars().count())
        } else {
            part.to_string()
        }
    }

    /// The whole text as painted, without newlines: the mouse hit test shapes this.
    fn display_all(&self) -> String {
        self.display(&self.buffer.text().replace(['\n', '\r'], " "))
    }

    fn insert_str(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.buffer.insert(text) {
            self.marked = None;
            self.changed(cx);
        }
    }

    fn replace_utf16(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let start = self.byte_of_utf16(range.start);
        let end = self.byte_of_utf16(range.end);
        self.buffer.move_to(start, false);
        self.buffer.move_to(end, true);
        self.buffer.insert(text);
        self.changed(cx);
    }

    fn byte_of_utf16(&self, offset: usize) -> usize {
        let mut units = 0;
        for (byte, character) in self.buffer.text().char_indices() {
            if units >= offset {
                return byte;
            }
            units += character.len_utf16();
        }
        self.buffer.text().len()
    }

    fn utf16_of_byte(&self, byte: usize) -> usize {
        self.buffer.text()[..byte.min(self.buffer.text().len())]
            .encode_utf16()
            .count()
    }

    fn copy_selection(&self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.buffer.selected_text().map(str::to_string) else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// The byte offset under a window-space x, for click-to-place and drag-select.
    fn index_at(&self, position: Point<Pixels>, window: &Window) -> usize {
        let display = self.display_all();
        if display.is_empty() {
            return 0;
        }
        let run = TextRun {
            len: display.len(),
            font: font(".SystemUIFont"),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(
            display.clone().into(),
            Theme::body_size(),
            &[run],
            None,
        );
        let x = position.x - self.text_origin.x;
        let byte = line.closest_index_for_x(if x < px(0.) { px(0.) } else { x });
        let byte = byte.min(display.len());
        if self.masked {
            // One dot per character: count characters, then find that boundary in the real text.
            let characters = display[..byte].chars().count();
            return self
                .buffer
                .text()
                .char_indices()
                .nth(characters)
                .map(|(offset, _)| offset)
                .unwrap_or(self.buffer.text().len());
        }
        self.buffer.floor_boundary(byte)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window);
        let index = self.index_at(event.position, window);
        self.buffer.move_to(index, event.modifiers.shift);
        self.dragging = true;
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.dragging || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let index = self.index_at(event.position, window);
        self.buffer.move_to(index, true);
        cx.notify();
    }

    fn on_mouse_up(&mut self, _event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.dragging {
            self.dragging = false;
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform {
            match event.keystroke.key.as_str() {
                "a" => {
                    self.buffer.select_all();
                    cx.notify();
                }
                "c" => {
                    self.copy_selection(cx);
                }
                "x" => {
                    if self.copy_selection(cx) && self.buffer.backspace() {
                        self.changed(cx);
                    }
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.insert_str(&text, cx);
                    }
                }
                _ => {}
            }
            return;
        }

        let extend = modifiers.shift;
        match event.keystroke.key.as_str() {
            "backspace" => {
                if self.buffer.backspace() {
                    self.changed(cx);
                }
            }
            "delete" => {
                if self.buffer.delete_forward() {
                    self.changed(cx);
                }
            }
            "left" => {
                self.buffer.move_left(extend);
                cx.notify();
            }
            "right" => {
                self.buffer.move_right(extend);
                cx.notify();
            }
            "home" => {
                self.buffer.move_home(extend);
                cx.notify();
            }
            "end" => {
                self.buffer.move_end(extend);
                cx.notify();
            }
            "enter" => {
                let text: SharedString = self.buffer.text().to_string().into();
                cx.emit(FieldSubmitted { text });
                window.blur();
            }
            _ => {}
        }
    }

    /// `before`, the selected run, `after` — the three spans the field paints in order.
    fn spans(&self) -> (String, String, String) {
        let text = self.buffer.text();
        match self.buffer.selection() {
            Some(range) => (
                self.display(&text[..range.start]),
                self.display(&text[range.clone()]),
                self.display(&text[range.end..]),
            ),
            None => {
                let caret = self.buffer.caret();
                (
                    self.display(&text[..caret]),
                    String::new(),
                    self.display(&text[caret..]),
                )
            }
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
        Some(self.buffer.text()[start..end].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let (start, end, reversed) = match self.buffer.selection() {
            Some(range) => (
                self.utf16_of_byte(range.start),
                self.utf16_of_byte(range.end),
                self.buffer.caret() == range.start,
            ),
            None => {
                let caret = self.utf16_of_byte(self.buffer.caret());
                (caret, caret, false)
            }
        };
        Some(UTF16Selection {
            range: start..end,
            reversed,
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
                let caret = self.utf16_of_byte(self.buffer.caret());
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
                let caret = self.utf16_of_byte(self.buffer.caret());
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
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.index_at(point, window))
    }
}

impl Render for TextField {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let focused = self.focus.is_focused(_window);
        let (before, selected, after) = self.spans();
        let empty = self.buffer.text().is_empty();
        let has_selection = !selected.is_empty();

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

        if empty && !focused {
            field = field.child(
                div()
                    .whitespace_nowrap()
                    .text_size(Theme::body_size())
                    .text_color(theme.on_surface_variant)
                    .child(self.placeholder.clone()),
            );
        } else {
            if !before.is_empty() {
                field = field.child(
                    div()
                        .whitespace_nowrap()
                        .text_size(Theme::body_size())
                        .text_color(theme.on_surface)
                        .child(before),
                );
            }
            if has_selection {
                field = field.child(
                    div()
                        .whitespace_nowrap()
                        .text_size(Theme::body_size())
                        .text_color(theme.on_surface)
                        .bg(theme.primary_container)
                        .child(selected),
                );
            } else if focused {
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
                        .child(after),
                );
            }
        }

        let field = field
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.on_mouse_down(event, window, cx)
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.on_mouse_move(event, window, cx)
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.on_mouse_up(event, window, cx)
                }),
            )
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
/// a plain `div` has no such hook. Painting also records where the text starts, which the mouse
/// hit test needs.
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
        // `px_3` on the field: the text starts after the left padding.
        let origin = point(bounds.origin.x + px(12.), bounds.origin.y);
        self.view
            .update(cx, |field, _cx| field.text_origin = origin);
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
