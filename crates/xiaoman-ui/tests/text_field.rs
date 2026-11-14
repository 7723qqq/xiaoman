//! The editing rules of the desktop text field: caret, selection, and what each key does.
//!
//! The field itself needs a window; [`Buffer`] is the pure half, so these pin the behaviour the
//! mouse and keyboard handlers rely on (Ctrl+A, Shift+arrows, Backspace over a selection).

use xiaoman_ui::views::text_field::Buffer;

#[test]
fn select_all_then_typing_replaces_everything() {
    let mut buffer = Buffer::new("sk-old-key");

    buffer.select_all();
    assert_eq!(buffer.selected_text(), Some("sk-old-key"));

    assert!(buffer.insert("sk-new"));
    assert_eq!(buffer.text(), "sk-new");
    assert_eq!(buffer.caret(), 6);
    assert_eq!(buffer.selection(), None);
}

#[test]
fn shift_arrows_grow_and_shrink_a_selection() {
    let mut buffer = Buffer::new("abcd");
    buffer.move_home(false);

    buffer.move_right(true);
    assert_eq!(buffer.selected_text(), Some("a"));
    buffer.move_right(true);
    assert_eq!(buffer.selected_text(), Some("ab"));
    buffer.move_left(true);
    assert_eq!(buffer.selected_text(), Some("a"));

    // Without Shift the caret collapses to the edge of the selection.
    buffer.move_left(false);
    assert_eq!(buffer.selection(), None);
    assert_eq!(buffer.caret(), 0);
}

#[test]
fn plain_arrows_collapse_a_selection_to_its_edges() {
    let mut buffer = Buffer::new("abcd");
    buffer.move_home(false);
    buffer.move_end(true);
    assert_eq!(buffer.selected_text(), Some("abcd"));

    buffer.move_left(false);
    assert_eq!(buffer.caret(), 0);

    buffer.move_end(true);
    assert_eq!(buffer.selected_text(), Some("abcd"));
    buffer.move_right(false);
    assert_eq!(buffer.caret(), 4, "collapse to the end of the selection");
}

#[test]
fn backspace_and_delete_remove_the_selection() {
    let mut buffer = Buffer::new("abcd");
    buffer.move_to(1, false);
    buffer.move_to(3, true);
    assert_eq!(buffer.selected_text(), Some("bc"));

    assert!(buffer.backspace());
    assert_eq!(buffer.text(), "ad");
    assert_eq!(buffer.caret(), 1);

    buffer.select_all();
    assert!(buffer.delete_forward());
    assert_eq!(buffer.text(), "");
    assert!(!buffer.backspace(), "nothing left to delete");
}

#[test]
fn a_caret_never_splits_a_character() {
    let mut buffer = Buffer::new("汉字");

    buffer.move_to(1, false);
    assert_eq!(buffer.caret(), 0, "inside 汉 (3 bytes) snaps back");
    buffer.move_to(4, false);
    assert_eq!(buffer.caret(), 3, "inside 字 snaps back to the boundary");
    buffer.move_to(999, false);
    assert_eq!(buffer.caret(), 6);

    buffer.move_home(false);
    buffer.move_right(false);
    assert_eq!(buffer.caret(), 3, "one character, not one byte");
}

#[test]
fn inserted_text_never_carries_newlines() {
    let mut buffer = Buffer::new("a");
    buffer.move_to(1, false);

    buffer.insert("b\nc\r");
    assert_eq!(buffer.text(), "ab c ");
}
