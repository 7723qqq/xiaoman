package dev.xiaoman.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dev.xiaoman.app.ChatRow
import dev.xiaoman.app.SessionStore
import dev.xiaoman.app.SessionUiState

/**
 * The chat screen: the transcript, the approval card, the pending question and the input row.
 *
 * The layout follows the Kotlin product's `ChatScreen`/`ChatTimeline`: progress lines are small
 * secondary text, the user's messages sit on the right, and the approval card is pinned above the
 * input instead of scrolling with the transcript.
 */
@Composable
fun ChatScreen(state: SessionUiState) {
    val listState = rememberLazyListState()
    var draft by remember { mutableStateOf("") }

    LaunchedEffect(state.rows.size, state.status) {
        if (state.rows.isNotEmpty()) listState.animateScrollToItem(state.rows.size - 1)
    }

    Column(Modifier.fillMaxSize()) {
        Header(state)
        LazyColumn(
            state = listState,
            modifier = Modifier.weight(1f).fillMaxWidth(),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (state.rows.isEmpty()) {
                item { EmptyChat() }
            }
            items(state.rows, key = { it.id }) { row -> ChatRowView(row) }
        }
        state.question?.let { question ->
            QuestionRow(
                text = question.text,
                takeover = question.takeover,
                onFocusInput = { /* the input below answers it */ },
            )
        }
        state.approval?.let { approval ->
            ApprovalCard(
                prompt = approval.prompt,
                reason = approval.reason,
                onOnce = { SessionStore.approve(approval.id, "once") },
                onTask = { SessionStore.approve(approval.id, "task") },
                onDeny = { SessionStore.deny(approval.id) },
            )
        }
        InputRow(
            draft = draft,
            onDraft = { draft = it },
            busy = state.busy,
            answering = state.question != null,
            enabled = state.configured || state.question != null,
            onSend = {
                val text = draft.trim()
                if (text.isNotEmpty() && SessionStore.send(text) != dev.xiaoman.app.RustSession.Outcome.BUSY) {
                    draft = ""
                }
            },
            onStop = { SessionStore.stop() },
        )
    }
}

@Composable
private fun Header(state: SessionUiState) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("小满", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.weight(1f))
            Text(
                when {
                    state.busy -> "处理中"
                    state.question != null -> "等你回答"
                    state.approval != null -> "等你确认"
                    else -> "就绪"
                },
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        state.status?.let {
            Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun EmptyChat() {
    Column(Modifier.fillMaxWidth().padding(vertical = 48.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Text("你可以让我记住事情，也可以让我替你操作手机。", style = MaterialTheme.typography.bodyMedium)
        Text(
            "先到「设置」里填写聊天模型和操作模型。",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun ChatRowView(row: ChatRow) {
    when (row.kind) {
        "user" -> Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.primaryContainer),
                shape = RoundedCornerShape(16.dp),
            ) {
                Text(row.text, Modifier.padding(horizontal = 12.dp, vertical = 8.dp), style = MaterialTheme.typography.bodyMedium)
            }
        }
        "assistant" -> Card(Modifier.fillMaxWidth()) {
            Text(row.text, Modifier.padding(12.dp), style = MaterialTheme.typography.bodyMedium)
        }
        "progress" -> Text(
            row.text,
            Modifier.fillMaxWidth().padding(horizontal = 4.dp),
            style = MaterialTheme.typography.bodySmall,
            color = if (row.ok == false) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
        )
        "question" -> Card(
            colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.secondaryContainer),
        ) {
            Text("小满问：", Modifier.padding(start = 12.dp, top = 8.dp), style = MaterialTheme.typography.labelMedium)
            Text(row.text, Modifier.padding(horizontal = 12.dp, vertical = 4.dp), style = MaterialTheme.typography.bodyMedium)
        }
        "error" -> Text(
            row.text,
            Modifier.fillMaxWidth().padding(horizontal = 4.dp),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.error,
        )
        else -> Text(row.text, Modifier.fillMaxWidth(), style = MaterialTheme.typography.bodySmall)
    }
}

@Composable
private fun QuestionRow(text: String, takeover: Boolean, onFocusInput: () -> Unit) {
    Card(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.secondaryContainer),
        onClick = onFocusInput,
    ) {
        Text(
            if (takeover) "请接管手机完成这一步，然后在这里回复" else "在下面回复",
            Modifier.padding(start = 12.dp, top = 8.dp),
            style = MaterialTheme.typography.labelMedium,
        )
        Text(text, Modifier.padding(horizontal = 12.dp, vertical = 4.dp), style = MaterialTheme.typography.bodySmall)
    }
}

@Composable
private fun ApprovalCard(
    prompt: String,
    reason: String,
    onOnce: () -> Unit,
    onTask: () -> Unit,
    onDeny: () -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer),
    ) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("需要你确认", style = MaterialTheme.typography.labelMedium)
            Text(prompt, style = MaterialTheme.typography.bodyMedium)
            Text(reason, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onErrorContainer)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = onDeny) { Text("暂不允许") }
                Button(onClick = onOnce) { Text("允许一次") }
                TextButton(onClick = onTask) { Text("本次都允许") }
            }
        }
    }
}

@Composable
private fun InputRow(
    draft: String,
    onDraft: (String) -> Unit,
    busy: Boolean,
    answering: Boolean,
    enabled: Boolean,
    onSend: () -> Unit,
    onStop: () -> Unit,
) {
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface).padding(12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(
                value = draft,
                onValueChange = onDraft,
                modifier = Modifier.weight(1f),
                placeholder = { Text(if (answering) "回答小满的问题" else "说点什么，或让我做一件事") },
                maxLines = 4,
                enabled = enabled,
            )
            Button(onClick = onSend, enabled = enabled && draft.isNotBlank()) { Text("发送") }
        }
        if (busy) {
            TextButton(onClick = onStop, modifier = Modifier.fillMaxWidth()) {
                Text("停止当前任务", color = MaterialTheme.colorScheme.error)
            }
        }
    }
}
