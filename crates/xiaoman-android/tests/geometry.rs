//! Port of `hands/src/test/kotlin/dev/xiaoman/hands/GeometryTest.kt`.

use xiaoman_android::geometry::{
    PxRect, Stroke, TAP_MS, denormalize, long_press_ms, looks_all_black, normalize_axis,
    normalize_rect, swipe_ms, target_size,
};
use xiaoman_api::{Bounds, Point};

fn point(x: i32, y: i32) -> Point {
    Point::new(x, y).expect("point in range")
}

#[test]
fn normalize_axis_scales_and_clamps() {
    assert_eq!(normalize_axis(0, 1080), 0);
    assert_eq!(normalize_axis(540, 1080), 500);
    assert_eq!(normalize_axis(1080, 1080), 1000);
    assert_eq!(normalize_axis(5000, 1080), 1000);
    assert_eq!(normalize_axis(-20, 1080), 0);
    assert_eq!(normalize_axis(10, 0), 0);
}

#[test]
fn normalize_rect_clamps_to_screen() {
    assert_eq!(
        normalize_rect(PxRect::new(-100, -50, 540, 600), 1080, 2400),
        Some(Bounds {
            left: 0,
            top: 0,
            right: 500,
            bottom: 250
        })
    );
    assert_eq!(
        normalize_rect(PxRect::new(540, 1200, 2000, 3000), 1080, 2400),
        Some(Bounds {
            left: 500,
            top: 500,
            right: 1000,
            bottom: 1000
        })
    );
}

#[test]
fn normalize_rect_offscreen_or_empty_is_none() {
    assert_eq!(
        normalize_rect(PxRect::new(0, 2500, 100, 2600), 1080, 2400),
        None
    );
    assert_eq!(
        normalize_rect(PxRect::new(10, 10, 10, 50), 1080, 2400),
        None
    );
    assert_eq!(normalize_rect(PxRect::new(0, 0, 10, 10), 0, 0), None);
}

#[test]
fn denormalize_keeps_the_far_edge_on_screen() {
    assert_eq!(denormalize(point(500, 500), 1080, 2400), (540.0, 1200.0));
    assert_eq!(denormalize(point(0, 0), 1080, 2400), (0.0, 0.0));
    // The far edge stays on screen.
    assert_eq!(denormalize(point(1000, 1000), 1080, 2400), (1079.0, 2399.0));
}

#[test]
fn round_trip_is_close() {
    let bounds = normalize_rect(PxRect::new(100, 300, 400, 500), 1080, 2400).expect("on screen");
    let (x, y) = denormalize(bounds.center(), 1080, 2400);
    assert!(
        (x - 250.0).abs() < 3.0 && (y - 400.0).abs() < 3.0,
        "({x}, {y})"
    );
}

#[test]
fn target_size_downscales_long_edge() {
    assert_eq!(target_size(1080, 2400), Ok((576, 1280)));
    assert_eq!(target_size(2400, 1080), Ok((1280, 576)));
    assert_eq!(target_size(1440, 1920), Ok((960, 1280)));
    assert_eq!(target_size(720, 1280), Ok((720, 1280)));
    assert_eq!(target_size(400, 800), Ok((400, 800)));
    assert_eq!(target_size(1, 100_000), Ok((1, 1280)));
}

#[test]
fn all_black_detection() {
    assert!(looks_all_black(&vec![0xFF00_0000u32; 100 * 200], 100, 200));
    let mut pixels = vec![0xFF00_0000u32; 100 * 200];
    pixels[..100 * 20].fill(0xFFFF_FFFF);
    assert!(!looks_all_black(&pixels, 100, 200));
    assert!(!looks_all_black(&[], 0, 0));
}

#[test]
fn tap_is_one_stroke_of_tap_duration() {
    let stroke = Stroke::tap(point(500, 500), 1080, 2400);
    assert_eq!(stroke.points, vec![(540.0, 1200.0)]);
    assert_eq!(stroke.duration_ms, TAP_MS);
}

#[test]
fn long_press_and_swipe_durations_are_clamped() {
    assert_eq!(long_press_ms(100), 400);
    assert_eq!(long_press_ms(800), 800);
    assert_eq!(long_press_ms(9_000), 5_000);
    assert_eq!(swipe_ms(10), 50);
    assert_eq!(swipe_ms(300), 300);
    assert_eq!(swipe_ms(99_999), 5_000);

    let press = Stroke::long_press(point(0, 0), 1080, 2400, 100);
    assert_eq!(press.points, vec![(0.0, 0.0)]);
    assert_eq!(press.duration_ms, 400);

    let swipe = Stroke::swipe(point(500, 900), point(500, 100), 1080, 2400, 10);
    assert_eq!(swipe.points, vec![(540.0, 2160.0), (540.0, 240.0)]);
    assert_eq!(swipe.duration_ms, 50);
}
