//! Screen geometry: pixel rectangles, the 0..=1000 normalized space used by [`Point`] and
//! [`Bounds`], screenshot downscaling, secure-screen detection and the tap/swipe helpers.
//!
//! Everything here is pure arithmetic; no Android types are involved, so it runs in unit tests on
//! the host and in the JNI layer on the phone alike.

use std::error::Error;
use std::fmt;

use xiaoman_api::{Bounds, NORM, Point};

/// A pixel rectangle in screen coordinates, same convention as `android.graphics.Rect`
/// (inclusive left/top, exclusive right/bottom).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PxRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PxRect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub const fn width(self) -> i32 {
        self.right - self.left
    }

    pub const fn height(self) -> i32 {
        self.bottom - self.top
    }

    pub fn center_x(self) -> f32 {
        (self.left + self.right) as f32 / 2.0
    }

    pub fn center_y(self) -> f32 {
        (self.top + self.bottom) as f32 / 2.0
    }

    /// The touch point of a node: the center of its pixel bounds.
    pub fn center(self) -> (f32, f32) {
        (self.center_x(), self.center_y())
    }
}

/// Pixel offset `px` along an axis of `extent_px` pixels to 0..=1000, clamped.
pub fn normalize_axis(px: i32, extent_px: i32) -> i32 {
    if extent_px <= 0 {
        return 0;
    }
    let scaled = i64::from(px) * i64::from(NORM) / i64::from(extent_px);
    scaled.clamp(0, i64::from(NORM)) as i32
}

/// Pixel rectangle to normalized bounds, clamped to the screen. `None` when nothing of the
/// rectangle is on screen or it has no area after clamping.
pub fn normalize_rect(rect: PxRect, width_px: i32, height_px: i32) -> Option<Bounds> {
    if width_px <= 0 || height_px <= 0 {
        return None;
    }
    let left = rect.left.clamp(0, width_px);
    let top = rect.top.clamp(0, height_px);
    let right = rect.right.clamp(0, width_px);
    let bottom = rect.bottom.clamp(0, height_px);
    if right <= left || bottom <= top {
        return None;
    }
    Some(Bounds {
        left: normalize_axis(left, width_px),
        top: normalize_axis(top, height_px),
        right: normalize_axis(right, width_px),
        bottom: normalize_axis(bottom, height_px),
    })
}

/// Normalized point to pixel coordinates, kept strictly inside the screen.
pub fn denormalize(point: Point, width_px: i32, height_px: i32) -> (f32, f32) {
    let max_x = (width_px - 1).max(0) as f32;
    let max_y = (height_px - 1).max(0) as f32;
    let x = (point.x() as f32 * width_px as f32 / NORM as f32).clamp(0.0, max_x);
    let y = (point.y() as f32 * height_px as f32 / NORM as f32).clamp(0.0, max_y);
    (x, y)
}

/// An image with no pixels; [`target_size`] refuses it instead of guessing a size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyImage {
    pub width_px: i32,
    pub height_px: i32,
}

impl fmt::Display for EmptyImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "empty image: {}x{}", self.width_px, self.height_px)
    }
}

impl Error for EmptyImage {}

/// Long edge of the screenshot sent to the operator model.
pub const DEFAULT_LONG_EDGE: i32 = 1280;

/// How many samples [`looks_all_black`] takes per axis.
pub const DEFAULT_BLACK_SAMPLES: usize = 16;

/// Target size keeping the aspect ratio with the long edge at most [`DEFAULT_LONG_EDGE`]; never
/// upscales.
pub fn target_size(width_px: i32, height_px: i32) -> Result<(i32, i32), EmptyImage> {
    target_size_capped(width_px, height_px, DEFAULT_LONG_EDGE)
}

/// [`target_size`] with an explicit long edge.
pub fn target_size_capped(
    width_px: i32,
    height_px: i32,
    max_long_edge: i32,
) -> Result<(i32, i32), EmptyImage> {
    if width_px <= 0 || height_px <= 0 {
        return Err(EmptyImage {
            width_px,
            height_px,
        });
    }
    let long = width_px.max(height_px);
    if long <= max_long_edge {
        return Ok((width_px, height_px));
    }
    let scale = f64::from(max_long_edge) / f64::from(long);
    let width = (f64::from(width_px) * scale).round().max(1.0) as i32;
    let height = (f64::from(height_px) * scale).round().max(1.0) as i32;
    Ok((width, height))
}

/// True when every sampled pixel is opaque pure black. On Android 11–13 a `FLAG_SECURE` window is
/// captured as black instead of failing with `ERROR_TAKE_SCREENSHOT_SECURE_WINDOW`.
///
/// `pixels` are ARGB, row-major, `width` per row; the alpha channel is ignored.
pub fn looks_all_black(pixels: &[u32], width: usize, height: usize) -> bool {
    looks_all_black_sampled(pixels, width, height, DEFAULT_BLACK_SAMPLES)
}

/// [`looks_all_black`] with an explicit sample grid.
pub fn looks_all_black_sampled(
    pixels: &[u32],
    width: usize,
    height: usize,
    samples_per_axis: usize,
) -> bool {
    let Some(area) = width.checked_mul(height) else {
        return false;
    };
    if width == 0 || height == 0 || pixels.len() < area {
        return false;
    }
    if samples_per_axis == 0 {
        return true;
    }
    for sample_y in 0..samples_per_axis {
        let y = sample_coord(sample_y, height, samples_per_axis);
        for sample_x in 0..samples_per_axis {
            let x = sample_coord(sample_x, width, samples_per_axis);
            if pixels[y * width + x] & 0x00FF_FFFF != 0 {
                return false;
            }
        }
    }
    true
}

fn sample_coord(index: usize, extent: usize, samples: usize) -> usize {
    let raw = (index as f64 + 0.5) * extent as f64 / samples as f64;
    (raw as usize).min(extent - 1)
}

/// Duration of a tap; a plain touch down and up.
pub const TAP_MS: i64 = 60;

/// Shortest and longest long press accepted from the model.
pub const LONG_PRESS_MS: (i64, i64) = (400, 5_000);

/// Shortest and longest swipe accepted from the model.
pub const SWIPE_MS: (i64, i64) = (50, 5_000);

/// Clamps a requested long press to [`LONG_PRESS_MS`].
pub fn long_press_ms(requested_ms: i64) -> i64 {
    requested_ms.clamp(LONG_PRESS_MS.0, LONG_PRESS_MS.1)
}

/// Clamps a requested swipe to [`SWIPE_MS`].
pub fn swipe_ms(requested_ms: i64) -> i64 {
    requested_ms.clamp(SWIPE_MS.0, SWIPE_MS.1)
}

/// One touch gesture in screen pixels, ready to become an `android.accessibilityservice`
/// `GestureDescription`: a single point is a tap or long press, two points are a swipe. The Java
/// side turns [`Stroke::points`] into `Path.moveTo`/`lineTo` and [`Stroke::duration_ms`] into
/// `StrokeDescription(path, 0, duration)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub points: Vec<(f32, f32)>,
    pub duration_ms: i64,
}

impl Stroke {
    /// Tap at a normalized point.
    pub fn tap(point: Point, width_px: i32, height_px: i32) -> Self {
        let (x, y) = denormalize(point, width_px, height_px);
        Self::tap_at(x, y)
    }

    /// Tap at a pixel coordinate, e.g. the center of a node's [`PxRect`]. The coordinate is used
    /// as given: it stays on screen because [`PxRect`] came from the phone.
    pub fn tap_at(x: f32, y: f32) -> Self {
        Self {
            points: vec![(x, y)],
            duration_ms: TAP_MS,
        }
    }

    /// Long press at a normalized point; `requested_ms` is clamped to [`LONG_PRESS_MS`].
    pub fn long_press(point: Point, width_px: i32, height_px: i32, requested_ms: i64) -> Self {
        let (x, y) = denormalize(point, width_px, height_px);
        Self::long_press_at(x, y, requested_ms)
    }

    /// Long press at a pixel coordinate; `requested_ms` is clamped to [`LONG_PRESS_MS`].
    pub fn long_press_at(x: f32, y: f32, requested_ms: i64) -> Self {
        Self {
            points: vec![(x, y)],
            duration_ms: long_press_ms(requested_ms),
        }
    }

    /// Swipe between two normalized points; `requested_ms` is clamped to [`SWIPE_MS`].
    pub fn swipe(from: Point, to: Point, width_px: i32, height_px: i32, requested_ms: i64) -> Self {
        let start = denormalize(from, width_px, height_px);
        let end = denormalize(to, width_px, height_px);
        Self {
            points: vec![start, end],
            duration_ms: swipe_ms(requested_ms),
        }
    }
}
