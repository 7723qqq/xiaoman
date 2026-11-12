//! Turns an accessibility tree into the compact node list handed to the operator model.
//!
//! Pre-order traversal; a node is kept when it is visible to the user and carries text or a
//! description, or can be clicked, long-clicked, edited or scrolled. Bounds are normalized to
//! 0..=1000 and the traversal stops once `max_nodes` nodes were kept, so the earlier a node comes
//! in pre-order the likelier it survives. There is no separate scoring pass: a container that is
//! only interesting because it scrolls (`ScrollView`, `RecyclerView`) keeps its position ahead of
//! the children it scrolls.

use xiaoman_api::{Bounds, UiNode};

use crate::geometry::{PxRect, normalize_rect};

/// Cap of the compact list, used by [`NodeCompactor::new`].
pub const DEFAULT_MAX_NODES: usize = 200;

/// Cap of one text or description, in characters, used by [`NodeCompactor::new`].
pub const DEFAULT_MAX_TEXT: usize = 200;

/// How deep the traversal descends, used by [`NodeCompactor::new`].
pub const DEFAULT_MAX_DEPTH: usize = 64;

/// The facts about one accessibility node that the compactor needs; independent of Android.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFacts {
    pub text: Option<String>,
    pub description: Option<String>,
    pub resource_id: Option<String>,
    pub class_name: Option<String>,
    pub bounds_px: PxRect,
    pub visible: bool,
    pub clickable: bool,
    pub long_clickable: bool,
    pub editable: bool,
    pub scrollable: bool,
    pub focused: bool,
    pub password: bool,
}

impl Default for NodeFacts {
    /// An unnamed, invisible-to-nobody node: visible, not actionable, with empty bounds.
    fn default() -> Self {
        Self {
            text: None,
            description: None,
            resource_id: None,
            class_name: None,
            bounds_px: PxRect::default(),
            visible: true,
            clickable: false,
            long_clickable: false,
            editable: false,
            scrollable: false,
            focused: false,
            password: false,
        }
    }
}

impl NodeFacts {
    /// Facts of a visible node with the given pixel bounds and nothing else set.
    pub fn new(bounds_px: PxRect) -> Self {
        Self {
            bounds_px,
            ..Self::default()
        }
    }
}

/// The description the compactor sees: the content description, else for an empty editable field
/// its hint text. Empty editable fields usually expose their placeholder only as hint text.
pub fn description_or_hint(
    content_description: Option<&str>,
    hint_text: Option<&str>,
    text: Option<&str>,
    editable: bool,
) -> Option<String> {
    if let Some(description) = content_description {
        return Some(description.to_string());
    }
    if editable && text.is_none_or(str::is_empty) {
        return hint_text.map(str::to_string);
    }
    None
}

/// Adapts a tree of `T` (`AccessibilityNodeInfo` on the phone, plain objects in tests).
pub trait NodeTree<T> {
    fn facts(&self, node: &T) -> NodeFacts;

    fn child_count(&self, node: &T) -> usize;

    /// May return `None` for children that vanished meanwhile.
    fn child(&self, node: &T, index: usize) -> Option<T>;

    /// Called for every node obtained from [`NodeTree::child`] that was not kept in the result.
    fn release(&self, _node: T) {}
}

/// A kept node: the compact [`UiNode`] plus the original node and its pixel bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactNode<T> {
    pub ui: UiNode,
    pub source: T,
    pub bounds_px: PxRect,
}

/// Compacts accessibility trees; ids of the kept [`UiNode`]s are their list indexes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeCompactor {
    max_nodes: usize,
    max_text_length: usize,
    max_depth: usize,
}

impl Default for NodeCompactor {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeCompactor {
    pub fn new() -> Self {
        Self::with_limits(DEFAULT_MAX_NODES, DEFAULT_MAX_TEXT, DEFAULT_MAX_DEPTH)
    }

    /// Default text and depth limits with a different node cap, e.g. an `AgentConfig::max_nodes`.
    pub fn with_max_nodes(max_nodes: usize) -> Self {
        Self::with_limits(max_nodes, DEFAULT_MAX_TEXT, DEFAULT_MAX_DEPTH)
    }

    /// Default node and depth limits with a different text cap.
    pub fn with_max_text_length(max_text_length: usize) -> Self {
        Self::with_limits(DEFAULT_MAX_NODES, max_text_length, DEFAULT_MAX_DEPTH)
    }

    pub fn with_limits(max_nodes: usize, max_text_length: usize, max_depth: usize) -> Self {
        Self {
            max_nodes,
            max_text_length,
            max_depth,
        }
    }

    pub fn max_nodes(&self) -> usize {
        self.max_nodes
    }

    pub fn max_text_length(&self) -> usize {
        self.max_text_length
    }

    pub fn max_depth(&self) -> usize {
        self.max_depth
    }

    /// The kept nodes of `roots` in pre-order. Roots that are not kept are the caller's to
    /// release; every child that is not kept goes to [`NodeTree::release`].
    pub fn compact<T: Clone>(
        &self,
        roots: &[T],
        tree: &dyn NodeTree<T>,
        width_px: i32,
        height_px: i32,
    ) -> Vec<CompactNode<T>> {
        let mut out = Vec::new();
        for root in roots {
            if out.len() >= self.max_nodes {
                break;
            }
            self.visit(root, 0, tree, width_px, height_px, &mut out);
        }
        out
    }

    /// Returns true when `node` was kept (so the caller must not release it).
    fn visit<T: Clone>(
        &self,
        node: &T,
        depth: usize,
        tree: &dyn NodeTree<T>,
        width_px: i32,
        height_px: i32,
        out: &mut Vec<CompactNode<T>>,
    ) -> bool {
        let facts = tree.facts(node);
        let mut kept = false;
        if facts.visible && is_interesting(&facts) {
            if let Some(bounds) = normalize_rect(facts.bounds_px, width_px, height_px) {
                let id = out.len() as i32;
                out.push(CompactNode {
                    ui: self.to_ui_node(id, &facts, bounds),
                    source: node.clone(),
                    bounds_px: facts.bounds_px,
                });
                kept = true;
            }
        }
        // Invisible containers can still hold visible children (e.g. clipped scroll content); keep
        // walking.
        if depth < self.max_depth {
            let count = tree.child_count(node);
            for index in 0..count {
                if out.len() >= self.max_nodes {
                    break;
                }
                let Some(child) = tree.child(node, index) else {
                    continue;
                };
                if !self.visit(&child, depth + 1, tree, width_px, height_px, out) {
                    tree.release(child);
                }
            }
        }
        kept
    }

    fn to_ui_node(&self, id: i32, facts: &NodeFacts, bounds: Bounds) -> UiNode {
        UiNode {
            id,
            // Never send the content of password fields to the model.
            text: if facts.password {
                None
            } else {
                self.clean(facts.text.as_deref())
            },
            description: self.clean(facts.description.as_deref()),
            resource_id: non_blank(facts.resource_id.as_deref()),
            class_name: facts.class_name.as_deref().and_then(short_class_name),
            bounds,
            clickable: facts.clickable || facts.long_clickable,
            editable: facts.editable,
            scrollable: facts.scrollable,
            focused: facts.focused,
            password: facts.password,
        }
    }

    fn clean(&self, value: Option<&str>) -> Option<String> {
        let value = value?;
        let collapsed = collapse_whitespace(value);
        let trimmed = collapsed.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed.chars().count() > self.max_text_length {
            let mut shortened: String = trimmed.chars().take(self.max_text_length).collect();
            shortened.push('…');
            return Some(shortened);
        }
        Some(trimmed.to_string())
    }
}

fn is_interesting(facts: &NodeFacts) -> bool {
    !is_blank(facts.text.as_deref())
        || !is_blank(facts.description.as_deref())
        || facts.clickable
        || facts.long_clickable
        || facts.editable
        || facts.scrollable
}

fn is_blank(value: Option<&str>) -> bool {
    value.is_none_or(|text| text.trim().is_empty())
}

fn non_blank(value: Option<&str>) -> Option<String> {
    value
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
}

/// `android.widget.Button` -> `Button`, as the model sees it.
fn short_class_name(name: &str) -> Option<String> {
    let short = name.rsplit('.').next().unwrap_or(name);
    if short.trim().is_empty() {
        None
    } else {
        Some(short.to_string())
    }
}

/// Collapses runs of ASCII whitespace to one space. The Kotlin original used `Regex("\\s+")`
/// with Java's ASCII-only `\s`, so wide spaces and no-break spaces survive to the `trim` below.
fn collapse_whitespace(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut in_run = false;
    for ch in value.chars() {
        if is_ascii_space(ch) {
            if !in_run {
                out.push(' ');
                in_run = true;
            }
        } else {
            out.push(ch);
            in_run = false;
        }
    }
    out
}

fn is_ascii_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r')
}
