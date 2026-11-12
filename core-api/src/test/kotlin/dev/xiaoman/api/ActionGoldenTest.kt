package dev.xiaoman.api

import java.io.File
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** The same fixtures `crates/xiaoman-api/tests/action_json.rs` reads; both sides must agree. */
class ActionGoldenTest {
    private val json = Json { classDiscriminator = "type"; ignoreUnknownKeys = true }

    @Test
    fun everyFixtureMatchesByteForByte() {
        val dir = File("../fixtures/action")
        val cases = dir.listFiles { file -> file.name.endsWith(".in.json") }!!.sortedBy { it.name }
        assertTrue(cases.size >= 10, "found ${cases.size} fixtures")
        for (file in cases) {
            val case = file.name.removeSuffix(".in.json")
            val action = json.decodeFromString<Action>(file.readText())
            val expected = File(dir, "$case.out.json").readText()
            assertEquals(expected, json.encodeToString(action), case)
        }
    }
}
