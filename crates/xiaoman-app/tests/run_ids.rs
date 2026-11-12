//! `RunIds.kt` has no Kotlin test file of its own; these pin the documented behaviour: the
//! `app-<yyyyMMdd-HHmmss>-<4 letters>` shape, priming for debug runs and the folder-name rule the
//! CLI shares.

use xiaoman_app::run_ids::{RunIds, format_stamp};

#[test]
fn ids_are_prefixed_stamped_in_the_given_zone_and_four_letters() {
    let mut ids = RunIds::with(|| 1_761_830_000_000, |_| 0, 480);
    assert_eq!(ids.generate(), "app-20251030-211320-aaaa");

    // The suffix walks the alphabet in order: a, b, c, d.
    let mut index = 0;
    let mut counting = RunIds::with(
        || 0,
        move |_| {
            let value = index;
            index = (index + 1) % 36;
            value
        },
        0,
    );
    assert_eq!(counting.next_id(), "app-19700101-000000-abcd");
}

#[test]
fn stamps_match_the_kotlin_date_format() {
    // 1761830000000 ms is 2025-10-30T13:13:20Z.
    assert_eq!(format_stamp(1_761_830_000_000, 0), "20251030-131320");
    assert_eq!(format_stamp(1_761_830_000_000, 480), "20251030-211320");
    assert_eq!(format_stamp(0, 0), "19700101-000000");
    // Out of range offsets and instants fall back instead of panicking.
    assert_eq!(format_stamp(0, 2000), "0");
}

#[test]
fn priming_returns_that_id_once() {
    let mut ids = RunIds::with(|| 0, |_| 1, 0);
    ids.prime("r1");
    assert_eq!(ids.next_id(), "r1");
    assert_eq!(ids.next_id(), "app-19700101-000000-bbbb");
    ids.prime("r2");
    assert_eq!(ids.next_id(), "r2");
}

#[test]
fn validity_matches_the_folder_name_rule() {
    assert!(RunIds::is_valid("r1"));
    assert!(RunIds::is_valid("app-20261031-120000-a1b2"));
    assert!(RunIds::is_valid("a.b_c-d"));
    assert!(
        RunIds::is_valid(&"a".repeat(80)),
        "80 characters is the limit"
    );
    assert!(!RunIds::is_valid(&"a".repeat(81)));
    assert!(!RunIds::is_valid(""));
    assert!(!RunIds::is_valid("."));
    assert!(!RunIds::is_valid(".."));
    assert!(!RunIds::is_valid("a/b"));
    assert!(!RunIds::is_valid("a b"));
    assert!(!RunIds::is_valid("r1\n"));
    assert!(
        !RunIds::is_valid("轨迹"),
        "only ASCII letters, digits, dot, underscore and dash"
    );
}

#[test]
fn generated_ids_are_valid() {
    let mut ids = RunIds::new();
    for _ in 0..200 {
        let id = ids.next_id();
        assert!(RunIds::is_valid(&id), "{id}");
        assert!(id.starts_with("app-"), "{id}");
    }
}
