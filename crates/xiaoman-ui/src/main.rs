//! The desktop entry point: one window, themed for the system appearance, showing the chat screen.
//!
//! This is the whole "app" as far as this crate is concerned: there is no Android activity, no
//! background service and no notification channel here — those are the phone shell, not the UI.
//!
//! The session's data directory is `--data-dir <path>` (`--data-dir=<path>` also works), else the
//! OS default of [`SessionConfig::default_data_dir`]. Settings, memory, skills and traces all live
//! there, so starting the app twice continues the same conversation state.

use std::path::PathBuf;

use gpui::{App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

use gpui_kit::component::{Theme as ComponentTheme, ThemeMode};

use xiaoman_app::session::{Session, SessionConfig};
use xiaoman_ui::theme;
use xiaoman_ui::views::shell::RootView;

fn main() {
    let data_dir = data_dir();
    let session = match Session::new(SessionConfig::new(&data_dir)) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("无法打开数据目录 {}：{error}", data_dir.display());
            std::process::exit(1);
        }
    };

    gpui_platform::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        // Centre on the primary display: a remembered or cascaded origin can land off-screen on a
        // narrower monitor, which looks exactly like "the app did not start".
        let bounds = Bounds::centered(None, size(px(430.), px(900.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("小满".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let light = theme::is_light(window.appearance());
                theme::install(cx, light);
                // The inputs and their edit menu come from `gpui-component`: give it the same
                // light/dark choice as the app, or its fields paint a light palette on a dark page.
                ComponentTheme::change(
                    if light {
                        ThemeMode::Light
                    } else {
                        ThemeMode::Dark
                    },
                    Some(window),
                    cx,
                );
                let view = cx.new(|cx| RootView::new(window, cx, session));
                // The component library renders its overlays (the text edit menu, dialogs)
                // inside `Root`; without it they have nowhere to paint.
                cx.new(|cx| gpui_kit::component::Root::new(view, window, cx).bordered(false))
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}

/// `--data-dir <path>` / `--data-dir=<path>`, else the OS default.
fn data_dir() -> PathBuf {
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--data-dir" {
            if let Some(path) = arguments.next() {
                return PathBuf::from(path);
            }
        } else if let Some(path) = argument.strip_prefix("--data-dir=") {
            if !path.is_empty() {
                return PathBuf::from(path);
            }
        }
    }
    SessionConfig::default_data_dir()
}
