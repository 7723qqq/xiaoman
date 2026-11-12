//! The Android front-end, written in Rust with GPUI + `gpui-mobile`.
//!
//! Five screens (chat, skills, services, memory, settings) are GPUI views over the real
//! `xiaoman_app::Session`, the same session the desktop app and the Compose fallback use. The
//! crate is a workspace member and builds on a desktop for its tests; the Android entry point
//! (`android_main`) is compiled only for `target_os = "android"`.
//!
//! Build: `cargo build --release --target aarch64-linux-android` (the Gradle host in `android/`
//! runs it and packages `libxiaoman_gpui.so`).

#![allow(unsafe_code)]

#[cfg(target_os = "android")]
use std::path::PathBuf;
#[cfg(target_os = "android")]
use std::sync::Arc;

#[cfg(target_os = "android")]
use gpui::{App, AppContext, Application, WindowOptions};
#[cfg(target_os = "android")]
use xiaoman_app::session::Session;

mod app;
mod chat;
mod memory;
mod services;
mod settings;
mod skills;
mod ui;

pub use app::RootView;

/// The session `HostApplication` opened through JNI, or one opened here on the same data
/// directory (external files, `.../files`, where the CLI pushes skills and pulls traces).
#[cfg(target_os = "android")]
fn open_session(app: &android_activity::AndroidApp) -> Option<Arc<Session>> {
    if let Some(session) = xiaoman_android::session::current() {
        log::info!("session: reusing the one HostApplication opened");
        return Some(Arc::new(session));
    }
    let data_dir: PathBuf = app
        .external_data_path()
        .or_else(|| app.internal_data_path())
        .unwrap_or_else(|| PathBuf::from("/data/data/dev.xiaoman.host/files"));
    let bundled = data_dir.join("bundled-skills");
    let bundled = bundled.is_dir().then_some(bundled);
    log::info!("session: opening under {}", data_dir.display());
    match xiaoman_android::session::open(&data_dir, bundled.as_deref()) {
        Ok(()) => xiaoman_android::session::current().map(Arc::new),
        Err(error) => {
            log::error!("session: cannot open: {error}");
            None
        }
    }
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("XiaomanGpui"),
    );
    gpui_mobile::android::jni::install_panic_hook();
    log::info!("android_main: entered");

    let _platform = gpui_mobile::android::jni::init_platform(&app);
    let Some(shared) = gpui_mobile::android::jni::shared_platform() else {
        log::error!("android_main: no shared platform");
        return;
    };
    let session = open_session(&app);

    Application::with_platform(shared.into_rc()).run(move |cx: &mut App| {
        gpui_kit::init(cx);
        // gpui-mobile maps the Android back button to `escape`; the shell answers it.
        cx.bind_keys([gpui::KeyBinding::new("escape", app::Back, None)]);
        log::info!("Application::run: opening the chat window");
        let session = session.clone();
        let opened = cx.open_window(WindowOptions::default(), move |window, cx| {
            let view = cx.new(|cx| RootView::new(session, window, cx));
            // The component library hosts the touch-selection handles and the long-press
            // copy/paste menu in `Root`; without it those overlays have nowhere to render.
            cx.new(|cx| gpui_kit::component::Root::new(view, window, cx).bordered(false))
        });
        if let Err(error) = opened {
            log::error!("open_window failed: {error}");
        }
    });
    log::info!("android_main: event loop returned");
}
