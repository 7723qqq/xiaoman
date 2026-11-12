package dev.xiaoman.app

import android.content.Context
import android.util.Log
import dev.xiaoman.host.R
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import org.json.JSONArray
import org.json.JSONObject

/** One row of the chat list, the `xiaoman_app::chat_item::ChatItem` JSON view. */
data class ChatRow(
    val id: Long,
    val kind: String,
    val text: String,
    val ok: Boolean? = null,
    val takeover: Boolean = false,
)

data class PendingApproval(val id: Long, val prompt: String, val reason: String)

data class PendingQuestion(val id: Long, val text: String, val takeover: Boolean)

/** Everything the chat screen renders, decoded from `RustSession.nativeSnapshot()`. */
data class SessionUiState(
    val rows: List<ChatRow> = emptyList(),
    val busy: Boolean = false,
    val status: String? = null,
    val configured: Boolean = false,
    val approval: PendingApproval? = null,
    val question: PendingQuestion? = null,
)

/**
 * The single source of truth for the Compose screens: polls the Rust session's snapshot while a
 * screen is visible and forwards the user's answers back through [RustSession].
 *
 * Polling rather than JNI callbacks keeps the seam one-directional: the snapshot always carries
 * the whole transcript, so a missed poll cannot lose a row.
 */
object SessionStore {

    private const val TAG = "Xiaoman"

    private val _state = MutableStateFlow(SessionUiState())
    val state: StateFlow<SessionUiState> = _state

    @Volatile
    private var appContext: Context? = null

    /** Remembers the application context for the notification mirror; call once from the Activity. */
    fun attach(context: Context) {
        appContext = context.applicationContext
    }

    fun refresh() {
        val json = RustSession.snapshot() ?: return
        val parsed = runCatching { parseSnapshot(json) }.getOrElse {
            Log.w(TAG, "session: cannot parse snapshot: ${it.message}")
            return
        }
        _state.value = parsed
        mirror(parsed)
    }

    fun send(text: String): RustSession.Outcome {
        val outcome = RustSession.send(text)
        refresh()
        if (outcome == RustSession.Outcome.STARTED) {
            appContext?.let { AgentService.start(it) }
        }
        return outcome
    }

    fun answer(text: String): Boolean {
        val answered = RustSession.nativeAnswerQuestion(text)
        refresh()
        return answered
    }

    fun approve(approvalId: Long, scope: String): Boolean {
        val answered = RustSession.nativeApprove(approvalId, scope)
        if (answered) appContext?.let { Notifications.hideApproval(it) }
        refresh()
        return answered
    }

    fun deny(approvalId: Long): Boolean {
        val answered = RustSession.nativeDeny(approvalId)
        if (answered) appContext?.let { Notifications.hideApproval(it) }
        refresh()
        return answered
    }

    fun stop() {
        RustSession.nativeStop()
        appContext?.let {
            Notifications.hideApproval(it)
            Notifications.hideQuestion(it)
            AgentService.stop(it)
        }
        refresh()
    }

    /** Mirrors the two parked prompts into the notification shade, and the busy status into the service. */
    private fun mirror(state: SessionUiState) {
        val context = appContext ?: return
        state.approval?.let { Notifications.showApproval(context, it.id, it.prompt, it.reason) }
            ?: Notifications.hideApproval(context)
        state.question?.let { Notifications.showQuestion(context, it.text, it.takeover) }
            ?: Notifications.hideQuestion(context)
        if (state.busy) Notifications.updateStatus(context, state.status ?: context.getString(R.string.notification_running))
    }

    private fun parseSnapshot(json: String): SessionUiState {
        val root = JSONObject(json)
        val items = root.optJSONArray("items") ?: JSONArray()
        val rows = ArrayList<ChatRow>(items.length())
        for (index in 0 until items.length()) {
            val item = items.optJSONObject(index) ?: continue
            rows += ChatRow(
                id = item.optLong("id"),
                kind = item.optString("kind"),
                text = item.optString("text"),
                ok = if (item.has("ok") && !item.isNull("ok")) item.optBoolean("ok") else null,
                takeover = item.optBoolean("takeover"),
            )
        }
        val approval = root.optJSONObject("approval")?.let {
            PendingApproval(
                id = it.optLong("id"),
                prompt = it.optString("prompt"),
                reason = it.optString("reason"),
            )
        }
        val question = root.optJSONObject("question")?.let {
            PendingQuestion(
                id = it.optLong("id"),
                text = it.optString("text"),
                takeover = it.optBoolean("takeover"),
            )
        }
        return SessionUiState(
            rows = rows,
            busy = root.optBoolean("busy"),
            status = root.optString("status").takeIf { it.isNotBlank() && it != "null" },
            configured = root.optBoolean("configured"),
            approval = approval,
            question = question,
        )
    }
}
