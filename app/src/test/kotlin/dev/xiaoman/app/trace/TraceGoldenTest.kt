package dev.xiaoman.app.trace

import dev.xiaoman.api.Action
import dev.xiaoman.api.ActionResult
import dev.xiaoman.api.AgentEvent
import dev.xiaoman.api.Decision
import java.io.File
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** The same fixtures `crates/xiaoman-app/tests/trace_golden.rs` reads. */
class TraceGoldenTest {
    private val time = 1_761_830_000_000L

    private fun decision(obj: JsonObject): Decision = when (obj.getValue("kind").jsonPrimitive.content) {
        "allow" -> Decision.Allow(obj["reason"]?.jsonPrimitive?.content ?: "allowed")
        "ask" -> Decision.Ask(
            obj.getValue("prompt").jsonPrimitive.content,
            obj.getValue("reason").jsonPrimitive.content,
        )
        "deny" -> Decision.Deny(obj.getValue("reason").jsonPrimitive.content)
        else -> error("unknown decision kind")
    }

    private fun event(obj: JsonObject): AgentEvent {
        val taskId = "r1"
        val text = obj["text"]?.jsonPrimitive?.content
        val action = { TraceFormat.json.decodeFromString<Action>(obj.getValue("action").toString()) }
        return when (obj.getValue("kind").jsonPrimitive.content) {
            "Reply" -> AgentEvent.Reply(taskId, text!!)
            "SkillChosen" -> AgentEvent.SkillChosen(taskId, text!!)
            "PhoneTaskStarted" -> AgentEvent.PhoneTaskStarted(taskId, text!!, null)
            "StepPlanned" -> AgentEvent.StepPlanned(taskId, obj.getValue("step").jsonPrimitive.int, action())
            "StepExecuted" -> AgentEvent.StepExecuted(
                taskId,
                obj.getValue("step").jsonPrimitive.int,
                action(),
                ActionResult(obj.getValue("ok").jsonPrimitive.booleanOrNull ?: false, text),
            )
            "StepBlocked" -> AgentEvent.StepBlocked(
                taskId,
                obj.getValue("step").jsonPrimitive.int,
                action(),
                decision(obj.getValue("decision").jsonObject),
            )
            "NeedsUser" -> AgentEvent.NeedsUser(taskId, text!!, obj["takeover"]?.jsonPrimitive?.booleanOrNull ?: false)
            "MemoryUpdated" -> AgentEvent.MemoryUpdated(taskId, text!!)
            "ToolCalled" -> AgentEvent.ToolCalled(
                taskId,
                obj.getValue("tool").jsonPrimitive.content,
                obj.getValue("ok").jsonPrimitive.booleanOrNull ?: false,
                obj.getValue("summary").jsonPrimitive.content,
            )
            "PhoneTaskFinished" -> AgentEvent.PhoneTaskFinished(
                taskId,
                obj.getValue("ok").jsonPrimitive.booleanOrNull ?: false,
                text!!,
                obj.getValue("steps").jsonPrimitive.int,
            )
            "Failed" -> AgentEvent.Failed(taskId, text!!)
            else -> error("unknown event kind")
        }
    }

    @Test
    fun everyEventRendersTheSharedLine() {
        val dir = File("../fixtures/trace")
        val events = TraceFormat.json.parseToJsonElement(File(dir, "events.json").readText()).jsonArray
        val lines = File(dir, "lines.jsonl").readLines().filter { it.isNotBlank() }
        assertEquals(events.size, lines.size, "one line per event")
        assertTrue(events.size >= 11, "expected the shared trace fixtures")
        events.forEachIndexed { index, element ->
            val obj = element.jsonObject
            val decision = (obj["decision"] as? JsonPrimitive)?.takeIf { it.isString }?.content
            val line = TraceFormat.line(event(obj), time, decision).toString()
            assertEquals(lines[index], line, "event $index")
        }
    }
}
