package dev.xiaoman.hands

/**
 * What `captureScreen()` returns to Rust: the descriptor in `bridge.rs` is
 * `Ldev/xiaoman/hands/ScreenShot;`, and `jni_bridge.rs` reads it with `getPng()[B` and
 * `getSecure()Z`.
 *
 * These stay Kotlin `val`s so the JVM getters keep exactly those names. `png` is null when the
 * capture failed or the window is `FLAG_SECURE`; `secure` tells Rust which of the two it was, so a
 * black frame is reported as "cannot see this screen" instead of an empty screenshot.
 */
class ScreenShot(val png: ByteArray?, val secure: Boolean)
