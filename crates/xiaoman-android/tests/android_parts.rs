//! The portable half of the Android device layer.
//!
//! `deviceWithoutServiceFailsGracefully` and `installedAppsListsLauncherActivities` stay in the
//! Kotlin suite: they need a Robolectric `Context` and the JNI-backed device, which this pass does
//! not build.

use xiaoman_android::geometry::PxRect;
use xiaoman_android::node_compactor::{NodeCompactor, NodeFacts, NodeTree, description_or_hint};
use xiaoman_android::stop_signal::MutableStopSignal;
use xiaoman_api::{Bounds, StopSignal};

struct N {
    facts: NodeFacts,
    children: Vec<usize>,
}

/// A tree indexed by position; the Kotlin test used `AccessibilityNodeInfo` objects.
struct Tree(Vec<N>);

impl NodeTree<usize> for Tree {
    fn facts(&self, node: &usize) -> NodeFacts {
        self.0[*node].facts.clone()
    }

    fn child_count(&self, node: &usize) -> usize {
        self.0[*node].children.len()
    }

    fn child(&self, node: &usize, index: usize) -> Option<usize> {
        self.0[*node].children.get(index).copied()
    }
}

#[test]
fn accessibility_nodes_to_ui_nodes() {
    // `node(text, rect)` in the Kotlin test: visible, with the view id of the fixture.
    let node = |text: Option<&str>, rect: PxRect| {
        let mut facts = NodeFacts::new(rect);
        facts.text = text.map(str::to_string);
        facts.resource_id = Some("com.example:id/n".to_string());
        facts
    };

    let root = node(None, PxRect::new(0, 0, 1080, 2400));
    let mut search = node(None, PxRect::new(40, 240, 1040, 360));
    search.editable = true;
    search.class_name = Some("android.widget.EditText".to_string());
    // Empty editable fields expose their placeholder only as hint text.
    search.description = description_or_hint(None, Some("你要去哪儿"), None, true);
    let mut go = node(Some("快车"), PxRect::new(0, 2160, 540, 2400));
    go.clickable = true;
    go.class_name = Some("android.widget.TextView".to_string());

    let tree = Tree(vec![
        N {
            facts: root,
            children: vec![1, 2],
        },
        N {
            facts: search,
            children: vec![],
        },
        N {
            facts: go,
            children: vec![],
        },
    ]);

    let out = NodeCompactor::new().compact(&[0], &tree, 1080, 2400);

    assert_eq!(out.len(), 2);
    assert_eq!(out[0].ui.description.as_deref(), Some("你要去哪儿"));
    assert!(out[0].ui.editable);
    assert_eq!(out[0].ui.class_name.as_deref(), Some("EditText"));
    assert_eq!(out[0].ui.resource_id.as_deref(), Some("com.example:id/n"));
    assert_eq!(
        out[0].ui.bounds,
        Bounds {
            left: 37,
            top: 100,
            right: 962,
            bottom: 150
        }
    );
    assert_eq!(out[1].ui.text.as_deref(), Some("快车"));
    assert!(out[1].ui.clickable);
    assert_eq!(
        out[1].ui.bounds,
        Bounds {
            left: 0,
            top: 900,
            right: 500,
            bottom: 1000
        }
    );
}

#[test]
fn stop_signal() {
    let signal = MutableStopSignal::new();
    assert!(!signal.stopped());
    signal.stop();
    assert!(signal.stopped());
    signal.reset();
    assert!(!signal.stopped());
}

#[test]
fn stop_signal_is_shareable_across_threads() {
    fn assert_shareable<T: StopSignal + Send + Sync + 'static>() {}
    assert_shareable::<MutableStopSignal>();
}
