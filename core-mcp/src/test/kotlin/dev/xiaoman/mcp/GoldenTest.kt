package dev.xiaoman.mcp

import java.io.File
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** The same fixture `crates/xiaoman-mcp/tests/golden.rs` reads. */
class GoldenTest {
    @Test
    fun theSharedConfigRedactsToTheSameText() {
        val root = Json.parseToJsonElement(File("../fixtures/mcp/redaction.json").readText()).jsonObject
        val env = root.getValue("env").jsonObject.mapValues { it.value.jsonPrimitive.content }
        val config = McpConfig.parse(root.getValue("config").toString(), env)
        val warnings = root.getValue("warnings").jsonArray.map { it.jsonPrimitive.content }
        assertEquals(warnings, config.warnings)
        val redactor = SecretRedactor(config.servers)
        val texts = root.getValue("texts").jsonArray
        assertTrue(texts.size >= 5, "expected the shared redaction cases")
        for (case in texts) {
            val obj = case.jsonObject
            val input = obj.getValue("input").jsonPrimitive.content
            val output = obj.getValue("output").jsonPrimitive.content
            assertEquals(output, redactor.redact(input), input)
        }
    }
}
