package dev.xiaoman.memory

import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** The same fixtures `crates/xiaoman-memory/tests/golden.rs` reads; both sides must agree. */
class GoldenTest {
    @Test
    fun everyFixtureRendersByteForByte() {
        val dir = File("../fixtures/memory")
        val cases = dir.listFiles { file -> file.name.endsWith(".input.md") }!!.sortedBy { it.name }
        assertTrue(cases.size >= 4, "found ${cases.size} fixtures")
        for (file in cases) {
            val case = file.name.removeSuffix(".input.md")
            val input = file.readText()
            val expected = File(dir, "$case.rendered.md").readText()
            val permalink = Regex("(?m)^permalink: (.+)$")
                .find(input)?.groupValues?.get(1)?.trim()?.removeSurrounding("\"") ?: case
            val note = BasicMemoryFormat.parse(input, permalink, 0)
            assertEquals(expected, BasicMemoryFormat.render(note), case)
        }
    }
}
