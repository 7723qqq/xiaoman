package dev.xiaoman.hands

import android.util.Log

/**
 * Loads `libxiaoman_android.so`, the Rust half built from `crates/xiaoman-android` and copied into
 * `jniLibs` by the `cargoNdk` Gradle task.
 *
 * The load is attempted once and its outcome remembered. A missing or partially built library is
 * logged and reported through [loaded] instead of crashing while the user enables the service: the
 * Kotlin shell stays usable (the status screen shows the state) and Rust simply never receives its
 * callbacks.
 */
internal object NativeLibrary {
    const val TAG = "Xiaoman"

    private const val NAME = "xiaoman_gpui"

    val loaded: Boolean by lazy {
        try {
            System.loadLibrary(NAME)
            Log.i(TAG, "lib$NAME.so loaded")
            true
        } catch (e: UnsatisfiedLinkError) {
            Log.w(TAG, "lib$NAME.so is not available: ${e.message}")
            false
        }
    }
}
