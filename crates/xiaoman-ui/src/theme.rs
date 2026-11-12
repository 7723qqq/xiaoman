//! `Theme.kt` in GPUI terms: the same stable Xiaoman colors and the same type scale, as plain
//! values a view can read instead of a Material theme lookup.

use gpui::{FontWeight, Hsla, Pixels, px, rgb};

/// `Green = Color(0xFF1E6B52)`.
pub const GREEN: u32 = 0x1E6B52;

/// One color role of the Compose `ColorScheme`, resolved for light or dark.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub light: bool,
    pub primary: Hsla,
    pub on_primary: Hsla,
    pub primary_container: Hsla,
    pub on_primary_container: Hsla,
    pub secondary_container: Hsla,
    pub on_secondary_container: Hsla,
    pub background: Hsla,
    pub surface: Hsla,
    pub surface_container_low: Hsla,
    pub surface_variant: Hsla,
    pub on_surface: Hsla,
    pub on_surface_variant: Hsla,
    pub outline_variant: Hsla,
    pub error: Hsla,
    pub error_container: Hsla,
    pub on_error_container: Hsla,
}

impl Theme {
    /// `lightColorScheme(...)` from `Theme.kt`.
    pub fn light() -> Self {
        Self {
            light: true,
            primary: color(GREEN),
            on_primary: color(0xFFFFFF),
            primary_container: color(0xE1F0E8),
            on_primary_container: color(0x174332),
            secondary_container: color(0xEDF3EE),
            on_secondary_container: color(0x254735),
            background: color(0xF7F9F6),
            surface: color(0xFFFFFF),
            surface_container_low: color(0xF0F4EF),
            surface_variant: color(0xF0F4EF),
            on_surface: color(0x202C25),
            on_surface_variant: color(0x637168),
            outline_variant: color(0xE0E7DF),
            error: color(0xB3261E),
            error_container: color(0xF9DEDC),
            on_error_container: color(0x410E0B),
        }
    }

    /// `darkColorScheme(...)` from `Theme.kt`.
    pub fn dark() -> Self {
        Self {
            light: false,
            primary: color(0x8BD6B5),
            on_primary: color(0x11382A),
            primary_container: color(0x204D3C),
            on_primary_container: color(0xC8EBD9),
            secondary_container: color(0x26332C),
            on_secondary_container: color(0xD3E7DA),
            background: color(0x121815),
            surface: color(0x1B231F),
            surface_container_low: color(0x202A24),
            surface_variant: color(0x202A24),
            on_surface: color(0xE4ECE6),
            on_surface_variant: color(0x9FB1A6),
            outline_variant: color(0x35463C),
            error: color(0xF2B8B5),
            error_container: color(0x5C1A16),
            on_error_container: color(0xF9DEDC),
        }
    }

    pub fn for_light(light: bool) -> Self {
        if light { Self::light() } else { Self::dark() }
    }

    /// `bodyMedium`: 14sp / 22sp.
    pub fn body_size() -> Pixels {
        px(14.)
    }

    pub fn body_line_height() -> Pixels {
        px(22.)
    }

    /// `bodySmall`: 12sp / 18sp.
    pub fn small_size() -> Pixels {
        px(12.)
    }

    pub fn small_line_height() -> Pixels {
        px(18.)
    }

    /// `titleMedium`: 18sp / 24sp, semibold.
    pub fn title_size() -> Pixels {
        px(18.)
    }

    pub fn title_layout_height() -> Pixels {
        px(24.)
    }

    /// `titleSmall`: 15sp / 22sp, semibold.
    pub fn title_small_size() -> Pixels {
        px(15.)
    }

    pub fn title_small_line_height() -> Pixels {
        px(22.)
    }

    /// `labelSmall`: 11sp / 16sp.
    pub fn label_size() -> Pixels {
        px(11.)
    }

    pub fn label_line_height() -> Pixels {
        px(16.)
    }

    /// `headlineSmall`, used by the welcome title.
    pub fn headline_size() -> Pixels {
        px(24.)
    }

    pub const SEMIBOLD: FontWeight = FontWeight::SEMIBOLD;
    pub const MEDIUM: FontWeight = FontWeight::MEDIUM;
    pub const BOLD: FontWeight = FontWeight::BOLD;
}

impl Default for Theme {
    fn default() -> Self {
        Self::light()
    }
}

impl gpui::Global for Theme {}

/// The theme every view reads, so no screen has to thread colors through its constructor.
pub fn theme(cx: &gpui::App) -> Theme {
    cx.try_global::<Theme>()
        .copied()
        .unwrap_or_else(Theme::light)
}

/// `isSystemInDarkTheme`: light for `Light`/`VibrantLight`, dark otherwise.
pub fn is_light(appearance: gpui::WindowAppearance) -> bool {
    matches!(
        appearance,
        gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight
    )
}

/// Installs the theme for the current appearance.
pub fn install(cx: &mut gpui::App, light: bool) {
    cx.set_global(Theme::for_light(light));
}

/// `Color(0xFF...)`: an opaque sRGB color as GPUI's `Hsla`.
pub fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}
