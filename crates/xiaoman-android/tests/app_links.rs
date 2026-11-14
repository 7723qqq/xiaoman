//! App links and deeplinks.

use xiaoman_android::app_links::{ACTION_SET_ALARM, AppLinkError, link_for};

#[test]
fn alarm_link_opens_prefilled_new_alarm_page() {
    let link = link_for("xiaoman://alarm/set?hour=14&minute=5&label=喝水")
        .expect("a valid Xiaoman link")
        .expect("a Xiaoman link");
    assert_eq!(link.action(), ACTION_SET_ALARM);
    let alarm = link.alarm().expect("set alarm");
    assert_eq!(alarm.hour, 14);
    assert_eq!(alarm.minute, 5);
    assert_eq!(alarm.label.as_deref(), Some("喝水"));
    // The UI is never skipped: the operator checks the page and taps save.
    assert!(!alarm.skip_ui);
    // No component: the intent is implicit, any clock app may answer it.
}

#[test]
fn minute_defaults_to_zero_and_label_is_optional() {
    let link = link_for("XIAOMAN://alarm/set/?hour=7")
        .expect("a valid Xiaoman link")
        .expect("a Xiaoman link");
    let alarm = link.alarm().expect("set alarm");
    assert_eq!(alarm.minute, 0);
    assert_eq!(alarm.label, None);
}

#[test]
fn other_schemes_are_not_xiaoman_links() {
    assert_eq!(link_for("amapuri://route/plan/?dname=家"), Ok(None));
    assert_eq!(link_for("not a uri"), Ok(None));
}

#[test]
fn malformed_or_unknown_links_are_rejected() {
    assert_eq!(
        link_for("xiaoman://alarm/set?hour=24"),
        Err(AppLinkError::BadHour)
    );
    assert_eq!(
        link_for("xiaoman://alarm/set?minute=5"),
        Err(AppLinkError::BadHour)
    );
    assert_eq!(
        link_for("xiaoman://alarm/set?hour=8&minute=60"),
        Err(AppLinkError::BadMinute)
    );
    for uri in [
        "xiaoman://alarm/delete?hour=8",
        "xiaoman://call/dial?number=110",
    ] {
        match link_for(uri) {
            Err(AppLinkError::Unsupported { uri: rejected }) => assert_eq!(rejected, uri),
            other => panic!("{uri} should be unsupported, got {other:?}"),
        }
    }
}

#[test]
fn query_values_decode_like_android_uri() {
    let link = link_for("xiaoman://alarm/set?hour=%37&label=drink+water")
        .expect("a valid Xiaoman link")
        .expect("a Xiaoman link");
    let alarm = link.alarm().expect("set alarm");
    assert_eq!(alarm.hour, 7);
    assert_eq!(alarm.label.as_deref(), Some("drink water"));
}

#[test]
fn label_is_trimmed_and_capped() {
    let long = format!("xiaoman://alarm/set?hour=8&label={}", "喝".repeat(70));
    let link = link_for(&long)
        .expect("a valid Xiaoman link")
        .expect("a Xiaoman link");
    assert_eq!(
        link.alarm()
            .unwrap()
            .label
            .as_deref()
            .unwrap()
            .chars()
            .count(),
        60
    );

    let spaced = link_for("xiaoman://alarm/set?hour=8&label=%20%E5%96%9D%20")
        .unwrap()
        .unwrap();
    assert_eq!(spaced.alarm().unwrap().label.as_deref(), Some("喝"));
}
