package dev.xiaoman.llm

import dev.xiaoman.api.ChatMessage
import dev.xiaoman.api.ChatRequest
import dev.xiaoman.api.ChatRole
import dev.xiaoman.api.ContentPart
import dev.xiaoman.api.LlmException
import dev.xiaoman.api.ModelConfig
import dev.xiaoman.api.ModelRole
import java.io.File
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.int
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/** The same fixture `crates/xiaoman-llm/tests/golden.rs` reads. */
class GoldenTest {
    @Test
    fun retryAndFallbackMatchTheSharedScript() {
        val cases = Json.parseToJsonElement(File("../fixtures/llm/retry.json").readText()).jsonArray
        assertTrue(cases.size >= 4, "expected the shared LLM fixtures")
        for (element in cases) {
            val case = element.jsonObject
            val name = case.getValue("name").jsonPrimitive.content
            val server = MockWebServer()
            server.start()
            try {
                case.getValue("responses").jsonArray.forEach { response ->
                    val obj = response.jsonObject
                    val queued = MockResponse()
                        .setResponseCode(obj.getValue("status").jsonPrimitive.int)
                        .setBody(obj.getValue("body").jsonPrimitive.content)
                    obj["retryAfter"]?.jsonPrimitive?.contentOrNull?.let { queued.setHeader("Retry-After", it) }
                    server.enqueue(queued)
                }
                val sleeps = mutableListOf<Long>()
                val config = ModelConfig(
                    ModelRole.CHAT,
                    server.url("/api/v3").toString().trimEnd('/'),
                    "m",
                    "sk-test-123456",
                )
                val client = OpenAiCompatibleClient(
                    config,
                    maxRetries = case["maxRetries"]?.jsonPrimitive?.intOrNull ?: 2,
                    sleep = { sleeps += it },
                )
                val request = ChatRequest(
                    listOf(ChatMessage(ChatRole.USER, listOf(ContentPart.Text("hi")))),
                    jsonMode = case["jsonMode"]?.jsonPrimitive?.booleanOrNull ?: false,
                )
                val expected = case.getValue("expected").jsonObject
                val ok = expected.getValue("ok").jsonPrimitive.booleanOrNull == true
                if (ok) {
                    val response = runBlocking { client.complete(request) }
                    assertEquals(expected.getValue("text").jsonPrimitive.content, response.text, name)
                } else {
                    val error = assertFailsWith<LlmException>(name) { runBlocking { client.complete(request) } }
                    assertEquals(expected.getValue("retryable").jsonPrimitive.booleanOrNull, error.retryable, name)
                }
                assertEquals(expected.getValue("attempts").jsonPrimitive.int, server.requestCount, "$name attempts")
                expected["sleeps"]?.let { expectedSleeps ->
                    assertEquals(expectedSleeps.jsonArray.map { it.jsonPrimitive.long }, sleeps, "$name sleeps")
                }
                if (expected["lastRequestWithoutResponseFormat"]?.jsonPrimitive?.booleanOrNull == true) {
                    val first = server.takeRequest()
                    val last = server.takeRequest()
                    assertTrue(first.body.readUtf8().contains("response_format"), "$name: first asks for JSON")
                    assertTrue(!last.body.readUtf8().contains("response_format"), "$name: fallback dropped it")
                }
            } finally {
                server.shutdown()
            }
        }
    }
}
