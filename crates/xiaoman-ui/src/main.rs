//! The desktop entry point: one window, themed for the system appearance, showing the chat screen.
//!
//! This is the whole "app" as far as this crate is concerned: there is no Android activity, no
//! background service and no notification channel here — those are the phone shell, not the UI.
//!
//! The session's data directory is `--data-dir <path>` (`--data-dir=<path>` also works), else the
//! OS default of [`SessionConfig::default_data_dir`]. Settings, memory, skills and traces all live
//! there, so starting the app twice continues the same conversation state.

use std::path::PathBuf;

use gpui::{
    App, AppContext as _, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point,
    px, size,
};

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

    Application::new().run(move |cx: &mut App| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(120.), px(60.)),
                    size: size(px(430.), px(900.)),
                })),
                titlebar: Some(TitlebarOptions {
                    title: Some("小满".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                theme::install(cx, theme::is_light(window.appearance()));
                cx.new(|cx| RootView::new(cx, session))
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
