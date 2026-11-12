package dev.xiaoman.hands

/**
 * The debug entry point of the scripted see-and-touch run: one native call into
 * `libxiaoman_android.so`, reached by `adb shell am broadcast -a dev.xiaoman.smoke` through
 * `dev.xiaoman.app.SmokeReceiver`.
 *
 * The run drives the accessibility service end to end without a model: it observes, launches
 * Settings, taps the Display entry, then checks the Back and Home global actions. It blocks for
 * several seconds, so it must be called off the main thread.
 */
object Smoke {
    /** `()Ljava/lang/String;` — the run's one-line summary, also the last line it logs. */
    @JvmStatic
    external fun runSmoke(): String
}
