package dev.xiaoman.skills

import java.io.File
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** The same fixtures `crates/xiaoman-skills/tests/golden.rs` reads. */
class GoldenTest {
    @Test
    fun everyFixtureMatchesFieldForField() {
        val dir = File("../fixtures/skills")
        val cases = dir.listFiles { file -> file.name.endsWith(".skill.md") }!!.sortedBy { it.name }
        assertTrue(cases.size >= 8, "found ${cases.size} fixtures")
        for (file in cases) {
            val case = file.name.removeSuffix(".skill.md")
            val skillMd = file.readText()
            val flow = File(dir, "$case.flow.yaml").takeIf { it.isFile }?.readText()
            val (_, problems) = SkillParser.parseFolder(case, skillMd, flow, case)
            val expected = Json.parseToJsonElement(File(dir, "$case.problems.json").readText()).jsonArray
            assertEquals(expected.size, problems.size, "$case: $problems")
            expected.forEachIndexed { index, element ->
                val obj = element.jsonObject
                assertEquals(obj.getValue("source").jsonPrimitive.content, problems[index].source, "$case[$index].source")
                assertEquals(obj.getValue("line").jsonPrimitive.int, problems[index].line, "$case[$index].line")
                assertEquals(obj.getValue("message").jsonPrimitive.content, problems[index].message, "$case[$index].message")
            }
        }
    }
}
