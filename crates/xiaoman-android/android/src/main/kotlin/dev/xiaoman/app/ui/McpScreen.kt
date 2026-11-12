package dev.xiaoman.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.xiaoman.app.RustSession
import org.json.JSONArray
import org.json.JSONObject

private data class McpKey(val name: String, val label: String, val help: String?)
private data class McpServerRow(
    val id: String,
    val title: String,
    val description: String,
    val remote: Boolean,
    val enabled: Boolean,
    val keys: List<McpKey>,
    val values: Map<String, String>,
)
private data class CustomRow(val title: String, val url: String, val confirm: String)
private data class McpUi(
    val platform: String,
    val servers: List<McpServerRow>,
    val custom: List<CustomRow>,
    val statuses: Map<String, String> = emptyMap(),
)

/**
 * The MCP services screen: the catalog ticked per service, the keys each one needs, custom
 * Streamable HTTP servers, and the same one-shot connection test as the app's `/api/mcp/test`.
 *
 * Saving writes `mcp.json` and the shared secret file through the session, so the next chat turn
 * offers the tools immediately.
 */
@Composable
fun McpScreen() {
    var ui by remember { mutableStateOf<McpUi?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var customTitle by remember { mutableStateOf("") }
    var customUrl by remember { mutableStateOf("") }

    fun load() {
        val root = parseObject(RustSession.nativeMcp()) ?: return
        ui = parse(root)
    }

    LaunchedEffect(Unit) { load() }

    val current = ui ?: return

    fun payload(source: McpUi): JSONObject {
        val enabled = JSONArray()
        val keys = JSONObject()
        for (server in source.servers) {
            if (server.enabled) enabled.put(server.id)
            for (key in server.keys) {
                val value = server.values[key.name].orEmpty()
                if (value.isNotBlank()) keys.put(key.name, value)
            }
        }
        val custom = JSONArray()
        for (entry in source.custom) {
            custom.put(JSONObject().put("title", entry.title).put("url", entry.url).put("confirm", entry.confirm))
        }
        return JSONObject().put("enabled", enabled).put("keys", keys).put("custom", custom)
    }

    fun save() {
        message = if (RustSession.nativeSaveMcp(payload(current).toString())) "已保存" else "保存失败"
    }

    fun test() {
        val statuses = parseStatuses(parseArray(RustSession.nativeTestMcp(payload(current).toString())))
        ui = current.copy(statuses = statuses)
        message = if (statuses.isEmpty()) "这些服务不需要连接测试" else "测试完成"
    }

    Column(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("服务", style = MaterialTheme.typography.titleMedium)
            Text(
                "勾选后聊天模型就能调用这些外部服务；Key 只保存在本机。",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { save() }) { Text("保存") }
                OutlinedButton(onClick = { test() }) { Text("测试连接") }
            }
            message?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        }
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(current.servers, key = { it.id }) { server ->
                Card(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Text(server.title, style = MaterialTheme.typography.titleSmall)
                                Text(
                                    (if (server.remote) "远程" else "本地进程") + " · ${server.id}",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                            Switch(
                                checked = server.enabled,
                                onCheckedChange = { checked ->
                                    ui = current.copy(
                                        servers = current.servers.map {
                                            if (it.id == server.id) it.copy(enabled = checked) else it
                                        },
                                    )
                                },
                            )
                        }
                        Text(server.description, style = MaterialTheme.typography.bodySmall)
                        for (key in server.keys) {
                            OutlinedTextField(
                                value = server.values[key.name].orEmpty(),
                                onValueChange = { value ->
                                    ui = current.copy(
                                        servers = current.servers.map {
                                            if (it.id != server.id) it
                                            else it.copy(values = it.values + (key.name to value))
                                        },
                                    )
                                },
                                label = { Text(key.label.ifBlank { key.name }) },
                                modifier = Modifier.fillMaxWidth(),
                                singleLine = true,
                                enabled = server.enabled,
                            )
                        }
                        current.statuses[server.id]?.let {
                            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                }
            }
            item {
                Text("自定义服务", Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleSmall)
            }
            items(current.custom) { entry ->
                Card(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Text(entry.title, style = MaterialTheme.typography.titleSmall)
                                Text(
                                    entry.url,
                                    style = MaterialTheme.typography.bodySmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                            TextButton(onClick = {
                                ui = current.copy(custom = current.custom - entry)
                            }) { Text("删除", color = MaterialTheme.colorScheme.error) }
                        }
                    }
                }
            }
            item {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    OutlinedTextField(
                        value = customTitle,
                        onValueChange = { customTitle = it },
                        label = { Text("名称") },
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                    )
                    OutlinedTextField(
                        value = customUrl,
                        onValueChange = { customUrl = it },
                        label = { Text("Streamable HTTP 地址") },
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                    )
                    OutlinedButton(onClick = {
                        if (customTitle.isNotBlank() && customUrl.isNotBlank()) {
                            ui = current.copy(
                                custom = current.custom + CustomRow(customTitle.trim(), customUrl.trim(), "WRITES"),
                            )
                            customTitle = ""
                            customUrl = ""
                        }
                    }) { Text("添加服务") }
                }
            }
        }
    }
}

private fun parse(root: JSONObject): McpUi {
    val servers = root.optJSONArray("servers") ?: JSONArray()
    val rows = ArrayList<McpServerRow>(servers.length())
    for (index in 0 until servers.length()) {
        val item = servers.optJSONObject(index) ?: continue
        val keys = item.optJSONArray("keys") ?: JSONArray()
        val values = item.optJSONArray("values") ?: JSONArray()
        val valueMap = HashMap<String, String>()
        for (valueIndex in 0 until values.length()) {
            val value = values.optJSONObject(valueIndex) ?: continue
            valueMap[value.optString("name")] = value.optString("value")
        }
        rows += McpServerRow(
            id = item.optString("id"),
            title = item.optString("title"),
            description = item.optString("description"),
            remote = item.optBoolean("remote"),
            enabled = item.optBoolean("enabled"),
            keys = (0 until keys.length()).mapNotNull { keyIndex ->
                val key = keys.optJSONObject(keyIndex) ?: return@mapNotNull null
                McpKey(
                    name = key.optString("name"),
                    label = key.optString("label"),
                    help = if (key.has("help") && !key.isNull("help")) key.optString("help") else null,
                )
            },
            values = valueMap,
        )
    }
    val custom = root.optJSONArray("custom") ?: JSONArray()
    return McpUi(
        platform = root.optString("platform"),
        servers = rows,
        custom = (0 until custom.length()).mapNotNull { index ->
            val item = custom.optJSONObject(index) ?: return@mapNotNull null
            CustomRow(item.optString("title"), item.optString("url"), item.optString("confirm", "WRITES"))
        },
    )
}

private fun parseArray(text: String?): JSONArray? =
    text?.takeIf { it.isNotBlank() }?.let { runCatching { JSONArray(it) }.getOrNull() }

private fun parseStatuses(array: JSONArray?): Map<String, String> {
    if (array == null) return emptyMap()
    val result = HashMap<String, String>()
    for (index in 0 until array.length()) {
        val item = array.optJSONObject(index) ?: continue
        val name = item.optString("name")
        result[name] = if (item.optBoolean("connected")) {
            "已连接 · ${item.optInt("toolCount")} 个工具"
        } else {
            "连接失败：${item.optString("error").ifBlank { "未知错误" }}"
        }
    }
    return result
}
