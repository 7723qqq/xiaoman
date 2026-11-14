//! The node tree compactor.

use std::cell::RefCell;
use std::rc::Rc;

use xiaoman_android::geometry::PxRect;
use xiaoman_android::node_compactor::{CompactNode, NodeCompactor, NodeFacts, NodeTree};
use xiaoman_api::Bounds;

struct N {
    facts: NodeFacts,
    children: Vec<Rc<N>>,
}

/// The Kotlin test tree records released nodes; identity matters, so nodes are `Rc`.
#[derive(Default)]
struct Tree {
    released: RefCell<Vec<Rc<N>>>,
}

impl NodeTree<Rc<N>> for Tree {
    fn facts(&self, node: &Rc<N>) -> NodeFacts {
        node.facts.clone()
    }

    fn child_count(&self, node: &Rc<N>) -> usize {
        node.children.len()
    }

    fn child(&self, node: &Rc<N>, index: usize) -> Option<Rc<N>> {
        node.children.get(index).cloned()
    }

    fn release(&self, node: Rc<N>) {
        self.released.borrow_mut().push(node);
    }
}

/// The Kotlin `n(...)` helper: defaults are the node's, not `NodeFacts`'s.
#[derive(Default)]
struct Spec {
    text: Option<String>,
    desc: Option<String>,
    cls: Option<String>,
    rect: Option<PxRect>,
    hidden: bool,
    clickable: bool,
    editable: bool,
    scrollable: bool,
    password: bool,
}

impl Spec {
    fn node(self, children: Vec<Rc<N>>) -> Rc<N> {
        let mut facts = NodeFacts::new(self.rect.unwrap_or(PxRect::new(0, 0, 100, 100)));
        facts.text = self.text;
        facts.description = self.desc;
        facts.class_name = self.cls;
        facts.visible = !self.hidden;
        facts.clickable = self.clickable;
        facts.editable = self.editable;
        facts.scrollable = self.scrollable;
        facts.password = self.password;
        Rc::new(N { facts, children })
    }
}

fn pointers<T>(nodes: &[T], pointer: impl Fn(&T) -> *const N) -> Vec<*const N> {
    nodes.iter().map(pointer).collect()
}

fn kept(out: &[CompactNode<Rc<N>>]) -> Vec<*const N> {
    pointers(out, |node| Rc::as_ptr(&node.source))
}

fn texts(out: &[CompactNode<Rc<N>>]) -> Vec<String> {
    out.iter()
        .map(|node| node.ui.text.clone().unwrap_or_default())
        .collect()
}

#[test]
fn pre_order_keeps_only_interesting_visible_nodes() {
    let title = Spec {
        text: Some("打车".into()),
        ..Spec::default()
    }
    .node(vec![]);
    let hidden = Spec {
        text: Some("hidden".into()),
        hidden: true,
        ..Spec::default()
    }
    .node(vec![]);
    let button = Spec {
        clickable: true,
        rect: Some(PxRect::new(0, 1200, 1080, 1320)),
        cls: Some("android.widget.Button".into()),
        ..Spec::default()
    }
    .node(vec![]);
    let plain = Spec::default().node(vec![]);
    let list = Spec {
        scrollable: true,
        rect: Some(PxRect::new(0, 200, 1080, 2400)),
        ..Spec::default()
    }
    .node(vec![button.clone(), plain.clone()]);
    let root = Spec {
        rect: Some(PxRect::new(0, 0, 1080, 2400)),
        ..Spec::default()
    }
    .node(vec![title.clone(), hidden.clone(), list.clone()]);
    let tree = Tree::default();

    let out = NodeCompactor::new().compact(&[root], &tree, 1080, 2400);

    assert_eq!(
        kept(&out),
        vec![Rc::as_ptr(&title), Rc::as_ptr(&list), Rc::as_ptr(&button)]
    );
    assert_eq!(
        out.iter().map(|node| node.ui.id).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(out[0].ui.text.as_deref(), Some("打车"));
    assert!(out[1].ui.scrollable);
    assert_eq!(
        out[2].ui.bounds,
        Bounds {
            left: 0,
            top: 500,
            right: 1000,
            bottom: 550
        }
    );
    assert_eq!(out[2].ui.class_name.as_deref(), Some("Button"));
    assert!(out[2].ui.clickable);

    // Children not kept are released; the root is the caller's business.
    let mut released: Vec<*const N> = tree.released.borrow().iter().map(Rc::as_ptr).collect();
    let mut expected = vec![Rc::as_ptr(&hidden), Rc::as_ptr(&plain)];
    released.sort();
    expected.sort();
    assert_eq!(released, expected);
}

#[test]
fn invisible_container_children_are_still_visited() {
    let inner = Spec {
        text: Some("visible child".into()),
        ..Spec::default()
    }
    .node(vec![]);
    let root = Spec {
        hidden: true,
        ..Spec::default()
    }
    .node(vec![inner.clone()]);

    let out = NodeCompactor::new().compact(&[root], &Tree::default(), 1080, 2400);

    assert_eq!(kept(&out), vec![Rc::as_ptr(&inner)]);
}

#[test]
fn cap_at_max_nodes_across_roots() {
    let a = Spec::default().node(
        (0..5)
            .map(|index| {
                Spec {
                    text: Some(format!("a{index}")),
                    ..Spec::default()
                }
                .node(vec![])
            })
            .collect(),
    );
    let b = Spec::default().node(
        (0..5)
            .map(|index| {
                Spec {
                    text: Some(format!("b{index}")),
                    ..Spec::default()
                }
                .node(vec![])
            })
            .collect(),
    );

    let out = NodeCompactor::with_max_nodes(7).compact(&[a, b], &Tree::default(), 1080, 2400);

    assert_eq!(texts(&out), vec!["a0", "a1", "a2", "a3", "a4", "b0", "b1"]);
    assert_eq!(
        out.iter().map(|node| node.ui.id).collect::<Vec<_>>(),
        (0..7).collect::<Vec<_>>()
    );
}

#[test]
fn offscreen_nodes_dropped() {
    let below = Spec {
        text: Some("below".into()),
        rect: Some(PxRect::new(0, 2500, 100, 2600)),
        ..Spec::default()
    }
    .node(vec![]);

    let out = NodeCompactor::new().compact(&[below], &Tree::default(), 1080, 2400);

    assert!(out.is_empty());
}

#[test]
fn password_text_hidden_and_whitespace_collapsed() {
    let password = Spec {
        text: Some("secret".into()),
        editable: true,
        password: true,
        ..Spec::default()
    }
    .node(vec![]);
    let messy = Spec {
        text: Some("  hello\n\n  world  ".into()),
        desc: Some("   ".into()),
        ..Spec::default()
    }
    .node(vec![]);
    let long = Spec {
        text: Some("x".repeat(500)),
        ..Spec::default()
    }
    .node(vec![]);

    let out = NodeCompactor::with_max_text_length(10).compact(
        &[password, messy, long],
        &Tree::default(),
        1080,
        2400,
    );

    assert_eq!(out[0].ui.text, None);
    assert!(out[0].ui.password && out[0].ui.editable);
    assert_eq!(out[1].ui.text.as_deref(), Some("hello worl…"));
    assert_eq!(out[1].ui.description, None);
    assert_eq!(out[2].ui.text.as_deref().unwrap().chars().count(), 11);
    assert!(!out[2].ui.clickable);
}
