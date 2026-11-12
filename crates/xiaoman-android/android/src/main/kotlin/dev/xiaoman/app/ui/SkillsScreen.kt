package dev.xiaoman.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.xiaoman.app.RustSession
import org.json.JSONArray

private data class SkillRow(
    val name: String,
    val description: String,
    val channel: String,
    val apps: Int,
    val confirmBefore: Int,
    val flowSteps: Int,
)

/**
 * The skills screen: what is loaded, where it comes from, and the same reload the CLI's
 * `RELOAD_SKILLS` triggers. Problems are shown rather than silently dropped, like the app's
 * `logSkillProblems`.
 */
@Composable
fun SkillsScreen() {
    var skills by remember { mutableStateOf<List<SkillRow>>(emptyList()) }
    var problems by remember { mutableStateOf<List<String>>(emptyList()) }
    var message by remember { mutableStateOf<String?>(null) }

    fun reload() {
        val root = parseObject(RustSession.nativeReloadSkills())
        if (root == null) {
            message = "技能加载失败"
            return
        }
        skills = parseSkills(root.optJSONArray("skills"))
        problems = parseProblems(root.optJSONArray("problems"))
        message = "已加载 ${skills.size} 个技能" + if (problems.isEmpty()) "" else "，${problems.size} 个有问题"
    }

    LaunchedEffect(Unit) { reload() }

    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(16.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            Column {
                Text("技能", style = MaterialTheme.typography.titleMedium)
                Text(
                    "放在数据目录的 skills/ 下，或使用内置示例。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            OutlinedButton(onClick = { reload() }) { Text("重新加载") }
        }
        message?.let {
            Text(it, Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall)
        }
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (skills.isEmpty()) {
                item { Text("还没有技能。", Modifier.padding(8.dp)) }
            }
            items(skills) { skill ->
                Card(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(skill.name, style = MaterialTheme.typography.titleSmall)
                        Text(skill.description, style = MaterialTheme.typography.bodyMedium)
                        Text(
                            buildString {
                                append(skill.channel)
                                if (skill.apps > 0) append(" · ${skill.apps} 个应用")
                                if (skill.confirmBefore > 0) append(" · ${skill.confirmBefore} 个确认词")
                                if (skill.flowSteps > 0) append(" · ${skill.flowSteps} 个流程步骤")
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
            if (problems.isNotEmpty()) {
                item {
                    Text(
                        "加载问题",
                        Modifier.padding(top = 8.dp),
                        style = MaterialTheme.typography.titleSmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
                items(problems) { problem ->
                    Text(problem, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                }
            }
        }
    }
}

private fun parseSkills(array: JSONArray?): List<SkillRow> {
    if (array == null) return emptyList()
    return (0 until array.length()).mapNotNull { index ->
        val item = array.optJSONObject(index) ?: return@mapNotNull null
        SkillRow(
            name = item.optString("name"),
            description = item.optString("description"),
            channel = item.optString("channel"),
            apps = item.optJSONArray("apps")?.length() ?: 0,
            confirmBefore = item.optJSONArray("confirmBefore")?.length() ?: 0,
            flowSteps = item.optJSONObject("flow")?.optJSONArray("steps")?.length() ?: 0,
        )
    }
}

private fun parseProblems(array: JSONArray?): List<String> {
    if (array == null) return emptyList()
    return (0 until array.length()).mapNotNull { index ->
        val item = array.optJSONObject(index) ?: return@mapNotNull null
        val source = item.optString("source")
        val line = if (item.has("line") && !item.isNull("line")) ":${item.optInt("line")}" else ""
        "$source$line ${item.optString("message")}"
    }
}
