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
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import dev.xiaoman.app.RustSession
import org.json.JSONArray

private data class NoteRow(val title: String, val permalink: String, val snippet: String = "")
private data class OpenNote(val title: String, val permalink: String, val body: String)

/**
 * Memory: the note list, a Chinese-aware search, one note open at a time and real deletion.
 *
 * The notes are Basic Memory Markdown files; opening one shows the Markdown body (title,
 * Observations, Relations), which is what the desktop's `NoteView` renders.
 */
@Composable
fun MemoryScreen() {
    var notes by remember { mutableStateOf<List<NoteRow>>(emptyList()) }
    var query by remember { mutableStateOf("") }
    var opened by remember { mutableStateOf<OpenNote?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var searching by remember { mutableStateOf(false) }

    fun list() {
        searching = false
        val root = parseObject(RustSession.nativeMemoryList(null))
        notes = parseNotes(root?.optJSONArray("notes"))
        message = root?.optString("error")?.takeIf { it.isNotBlank() && it != "null" }
    }

    fun search() {
        val text = query.trim()
        if (text.isEmpty()) {
            list()
            return
        }
        val root = parseObject(RustSession.nativeMemorySearch(text, 30))
        val hits = root?.optJSONArray("hits")
        notes = parseNotes(hits)
        searching = true
        message = if (notes.isEmpty()) "没有找到「$text」" else "找到 ${notes.size} 条"
    }

    fun openNote(permalink: String) {
        val root = parseObject(RustSession.nativeMemoryRead(permalink)) ?: return
        val note = root.optJSONObject("note") ?: return
        opened = OpenNote(
            title = note.optString("title"),
            permalink = note.optString("permalink"),
            body = note.optString("body"),
        )
    }

    LaunchedEffect(Unit) { list() }

    val current = opened
    if (current != null) {
        Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                OutlinedButton(onClick = { opened = null }) { Text("返回") }
                TextButton(onClick = {
                    if (RustSession.nativeMemoryDelete(current.permalink)) {
                        message = "已删除「${current.title}」"
                        opened = null
                        list()
                    } else {
                        message = "删除失败"
                    }
                }) { Text("忘记", color = MaterialTheme.colorScheme.error) }
            }
            Text(current.title, style = MaterialTheme.typography.titleLarge)
            Text(
                current.permalink,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            LazyColumn(Modifier.fillMaxSize()) {
                item {
                    Text(
                        current.body.ifBlank { "（这条笔记还没有内容）" },
                        style = MaterialTheme.typography.bodyMedium,
                        fontFamily = FontFamily.Monospace,
                    )
                }
            }
        }
        return
    }

    Column(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("记忆", style = MaterialTheme.typography.titleMedium)
            Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = query,
                    onValueChange = { query = it },
                    modifier = Modifier.weight(1f),
                    placeholder = { Text("搜索记忆") },
                    singleLine = true,
                )
                Button(onClick = { search() }) { Text("搜索") }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = { list() }) { Text("全部") }
                message?.let {
                    Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
        LazyColumn(
            modifier = Modifier.fillMaxSize(),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (notes.isEmpty() && !searching) {
                item { Text("还没有记忆。告诉小满一些事，它会记下来。", Modifier.padding(8.dp)) }
            }
            items(notes) { note ->
                Card(modifier = Modifier.fillMaxWidth(), onClick = { openNote(note.permalink) }) {
                    Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(note.title, style = MaterialTheme.typography.titleSmall)
                        Text(
                            note.permalink,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        if (note.snippet.isNotBlank()) {
                            Text(note.snippet, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
        }
    }
}

private fun parseNotes(array: JSONArray?): List<NoteRow> {
    if (array == null) return emptyList()
    return (0 until array.length()).mapNotNull { index ->
        val item = array.optJSONObject(index) ?: return@mapNotNull null
        NoteRow(
            title = item.optString("title"),
            permalink = item.optString("permalink"),
            snippet = item.optString("snippet"),
        )
    }
}
