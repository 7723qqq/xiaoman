//! Guards the click dispatch of the desktop screens.
//!
//! GPUI 0.2 keys `pending_mouse_down` by `ElementId`: two interactive elements that share an id
//! also share that pending press, and the *first painted* one clears it during the capture phase
//! when the pointer is over the other — so the element actually clicked never fires its handler.
//! The visible symptom is "only the first switch on the page works" (see `fix(ui): 同 id 的控件
//! 只有第一个能点`). These checks keep that class of bug out of the code base:
//!
//! 1. every interactive helper takes an explicit `id` as its first parameter,
//! 2. no id is derived from the label text,
//! 3. no view calls a helper with the theme first (i.e. without an id),
//! 4. no literal id repeats within one screen.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// The screens the shell paints, one at a time.
const VIEWS: [&str; 7] = [
    "chat.rs",
    "mcp.rs",
    "memory.rs",
    "settings.rs",
    "shell.rs",
    "skills.rs",
    "vocab.rs",
];

const HELPERS: [&str; 6] = [
    "text_button",
    "filled_button",
    "outlined_button",
    "chip",
    "toggle",
    "banner",
];

fn views_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/views")
}

fn read(file: &str) -> String {
    let path: PathBuf = views_dir().join(file);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The text between the parenthesis at `open` and its match.
fn paren_content(text: &str, open: usize) -> &str {
    let mut depth = 0i32;
    for (offset, ch) in text[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced parentheses in:\n{}", &text[open..]);
}

#[test]
fn every_interactive_helper_takes_an_id_first() {
    let text = read("components.rs");
    for helper in HELPERS {
        let marker = format!("pub fn {helper}(");
        let start = text
            .find(&marker)
            .unwrap_or_else(|| panic!("`{helper}` is gone; update this list"));
        let params = paren_content(&text, start + marker.len() - 1);
        let first = params.split(',').next().unwrap_or_default().trim();
        assert_eq!(
            first, "id: impl Into<SharedString>",
            "`{helper}` must take an id first, found `{first}`; a label-derived id lets two \
             elements share one ElementId, which silently kills clicks"
        );
    }
}

#[test]
fn ids_are_not_derived_from_labels() {
    let text = read("components.rs");
    for pattern in [
        "format!(\"filled-",
        "format!(\"chip-",
        "format!(\"text-",
        "format!(\"outlined-",
        "id(\"switch\")",
        "id(\"banner-action\")",
    ] {
        assert!(
            !text.contains(pattern),
            "components.rs still builds an id from shared text: `{pattern}`"
        );
    }
}

#[test]
fn views_pass_an_id_instead_of_a_label() {
    for file in VIEWS {
        let text = read(file);
        for helper in HELPERS {
            for bad in [format!("{helper}(&theme"), format!("{helper}(theme")] {
                assert!(
                    !text.contains(&bad),
                    "{file}: `{bad}…)` passes no id; every interactive helper takes one first"
                );
            }
        }
    }
}

#[test]
fn literal_ids_are_unique_within_a_screen() {
    for file in VIEWS {
        let text = read(file);
        let mut ids: BTreeMap<String, usize> = BTreeMap::new();
        for chunk in text.split(".id(\"").skip(1) {
            let end = chunk.find('"').expect("a closing quote");
            *ids.entry(chunk[..end].to_string()).or_default() += 1;
        }
        for (id, count) in ids {
            assert_eq!(
                count, 1,
                "{file}: the id `{id}` is used {count} times; ids must be unique per screen"
            );
        }
    }
}
