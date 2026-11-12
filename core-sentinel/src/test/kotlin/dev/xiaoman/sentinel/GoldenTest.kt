package dev.xiaoman.sentinel

import dev.xiaoman.api.Action
import dev.xiaoman.api.ApprovalAnswer
import dev.xiaoman.api.AuditSink
import dev.xiaoman.api.Bounds
import dev.xiaoman.api.Decision
import dev.xiaoman.api.GuardContext
import dev.xiaoman.api.ScreenState
import dev.xiaoman.api.Skill
import dev.xiaoman.api.UiNode
import java.io.File
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertTrue

/** The same fixtures `crates/xiaoman-sentinel/tests/golden.rs` reads. */
class GoldenTest {
    private val json = Json { ignoreUnknownKeys = true }

    private fun skill(obj: JsonObject): Skill {
        fun strings(key: String): List<String> =
            obj[key]?.jsonArray?.map { it.jsonPrimitive.content } ?: emptyList()
        val deeplinks = obj["deeplinks"]?.jsonObject?.mapValues { it.value.jsonPrimitive.content } ?: emptyMap()
        return Skill(
            name = obj.getValue("name").jsonPrimitive.content,
            description = obj.getValue("description").jsonPrimitive.content,
            apps = strings("apps"),
            confirmBefore = strings("confirmBefore"),
            deeplinks = deeplinks,
            instructions = "",
            source = "fixture",
        )
    }

    private fun node(obj: JsonObject): UiNode {
        val bounds = obj.getValue("bounds").jsonObject
        return UiNode(
            id = obj.getValue("id").jsonPrimitive.int,
            text = obj["text"]?.jsonPrimitive?.contentOrNull,
            description = obj["description"]?.jsonPrimitive?.contentOrNull,
            bounds = Bounds(
                bounds.getValue("left").jsonPrimitive.int,
                bounds.getValue("top").jsonPrimitive.int,
                bounds.getValue("right").jsonPrimitive.int,
                bounds.getValue("bottom").jsonPrimitive.int,
            ),
            clickable = obj["clickable"]?.jsonPrimitive?.booleanOrNull ?: false,
            editable = obj["editable"]?.jsonPrimitive?.booleanOrNull ?: false,
            focused = obj["focused"]?.jsonPrimitive?.booleanOrNull ?: false,
            password = obj["password"]?.jsonPrimitive?.booleanOrNull ?: false,
        )
    }

    private fun screen(obj: JsonObject): ScreenState = ScreenState(
        screenshotPng = null,
        widthPx = 1080,
        heightPx = 2400,
        packageName = obj["packageName"]?.jsonPrimitive?.contentOrNull,
        activityName = obj["activityName"]?.jsonPrimitive?.contentOrNull,
        nodes = obj["nodes"]?.jsonArray?.map { node(it.jsonObject) } ?: emptyList(),
        secure = obj["secure"]?.jsonPrimitive?.booleanOrNull ?: false,
    )

    @Test
    fun everyCaseMatchesTheSharedDecision() = runTest {
        val dir = File("../fixtures/sentinel")
        val cases = dir.listFiles { file -> file.name.endsWith(".json") }!!.sortedBy { it.name }
        assertTrue(cases.size >= 15, "found ${cases.size} fixtures")
        var actions = 0
        for (file in cases) {
            val root = json.parseToJsonElement(file.readText()).jsonObject
            val skill = root["skill"]?.takeUnless { it is JsonNull }?.jsonObject?.let(::skill)
            val screen = root["screen"]?.takeUnless { it is JsonNull }?.jsonObject?.let(::screen)
            val sentinel = PolicySentinel({ ApprovalAnswer(false) }, AuditSink { }, clock = { 0L })
            root.getValue("actions").jsonArray.forEachIndexed { index, entry ->
                val action = json.decodeFromString<Action>(entry.jsonObject.getValue("action").toString())
                val decision = sentinel.guard(GuardContext("fixture", "fixture", skill, screen, 0), action)
                val expected = entry.jsonObject.getValue("decision").jsonObject
                val label = "${file.name}[$index]"
                when (expected.getValue("kind").jsonPrimitive.content) {
                    "allow" -> {
                        val allow = assertIs<Decision.Allow>(decision, label)
                        val reason = expected["reason"]?.jsonPrimitive?.content ?: "allowed"
                        assertEquals(reason, allow.reason, "$label allow reason")
                    }
                    "deny" -> {
                        val deny = assertIs<Decision.Deny>(decision, label)
                        assertEquals(expected.getValue("reason").jsonPrimitive.content, deny.reason, "$label deny reason")
                    }
                    "ask" -> {
                        val ask = assertIs<Decision.Ask>(decision, label)
                        assertEquals(expected.getValue("prompt").jsonPrimitive.content, ask.prompt, "$label prompt")
                        assertEquals(expected.getValue("reason").jsonPrimitive.content, ask.reason, "$label reason")
                    }
                    else -> error("$label: unknown decision kind")
                }
                actions++
            }
        }
        assertTrue(actions >= 15, "expected actions, found $actions")
    }
}
