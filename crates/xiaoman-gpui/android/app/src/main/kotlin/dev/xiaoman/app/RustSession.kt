package dev.xiaoman.app

import dev.xiaoman.hands.NativeLibrary

/**
 * The Kotlin side of the Rust session JNI seam (`crates/xiaoman-android/src/session.rs`).
 *
 * One session per process: [open] is called from [HostApplication], every other call is a thin
 * native call. Chat state comes back as JSON ([snapshot]) so the Compose screens never parse Rust
 * types; [snapshot] is polled while a screen is visible instead of pushing callbacks across JNI.
 */
object RustSession {

    external fun nativeOpen(dataDir: String, bundledSkillsDir: String?, notifier: Any): Boolean
    external fun nativeClose()
    external fun nativeSnapshot(): String?
    external fun nativeSend(text: String): String?
    external fun nativeRunTask(goal: String, skill: String?, runId: String?): Boolean
    external fun nativeFail(runId: String, message: String)
    external fun nativeApprove(approvalId: Long, scope: String): Boolean
    external fun nativeDeny(approvalId: Long): Boolean
    external fun nativeAnswerQuestion(text: String): Boolean
    external fun nativeStop()
    external fun nativeSettings(): String?
    external fun nativeSaveSettings(json: String): Boolean
    external fun nativeSkills(): String?
    external fun nativeReloadSkills(): String?
    external fun nativeMemoryList(folder: String?): String?
    external fun nativeMemorySearch(query: String, limit: Int): String?
    external fun nativeMemoryRead(identifier: String): String?
    external fun nativeMemoryDelete(identifier: String): Boolean
    external fun nativeMcp(): String?
    external fun nativeSaveMcp(json: String): Boolean
    external fun nativeTestMcp(json: String): String?
    external fun nativeTracesDir(): String?
    external fun nativeDataDir(): String?

    /** What [send] returns, the Kotlin `AgentSession.Submit` names. */
    enum class Outcome {
        STARTED, ANSWERED, BUSY, NOT_CONFIGURED, EMPTY;

        companion object {
            fun parse(text: String?): Outcome = when (text) {
                "started" -> STARTED
                "answered" -> ANSWERED
                "busy" -> BUSY
                "empty" -> EMPTY
                else -> NOT_CONFIGURED
            }
        }
    }

    /** True once [open] has a session behind it. */
    @Volatile
    var opened: Boolean = false
        private set

    /**
     * Opens the one session under [dataDir]; [bundledSkillsDir] is searched before the user's own
     * `skills/` folder. Returns false when the library is missing or the directory cannot be used.
     */
    fun open(dataDir: String, bundledSkillsDir: String?, notifier: Any): Boolean {
        if (!NativeLibrary.loaded) return false
        opened = runCatching { nativeOpen(dataDir, bundledSkillsDir, notifier) }.getOrElse { false }
        return opened
    }

    fun close() {
        if (!opened) return
        runCatching { nativeClose() }
        opened = false
    }

    /** The state the chat screen renders, or `null` when no session is open. */
    fun snapshot(): String? = if (opened) runCatching { nativeSnapshot() }.getOrNull() else null

    /**
     * `busy` and `status` of the session, or `null` when no session is open. The foreground service
     * polls this so it stops (and refreshes its status) even when no screen is visible.
     */
    fun busyStatus(): Pair<Boolean, String?>? {
        val json = snapshot() ?: return null
        val body = runCatching { org.json.JSONObject(json) }.getOrNull() ?: return null
        val status = if (body.isNull("status")) null else body.optString("status").takeIf { it.isNotBlank() }
        return body.optBoolean("busy") to status
    }

    fun send(text: String): Outcome {
        if (!opened) return Outcome.NOT_CONFIGURED
        val json = runCatching { nativeSend(text) }.getOrNull() ?: return Outcome.NOT_CONFIGURED
        val outcome = runCatching { org.json.JSONObject(json).optString("outcome") }.getOrNull()
        return Outcome.parse(outcome)
    }
}
