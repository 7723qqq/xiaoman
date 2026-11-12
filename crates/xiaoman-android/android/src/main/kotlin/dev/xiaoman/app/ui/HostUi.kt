package dev.xiaoman.app.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import dev.xiaoman.app.SessionStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** The five screens of the Kotlin product, in the same order as the desktop shell. */
enum class HostTab(val label: String) {
    Chat("聊天"),
    Skills("技能"),
    Services("服务"),
    Memory("记忆"),
    Settings("设置"),
}

/**
 * The app's root: the five tabs over one polled session.
 *
 * The poll is the only timer in the UI. It runs while the root is composed, so a background task
 * keeps writing its trace while the shade shows the status, and the screens always re-read the
 * session's own transcript instead of keeping a second copy.
 */
@Composable
fun HostUi() {
    val context = LocalContext.current
    val state by SessionStore.state.collectAsState()
    var tab by remember { mutableStateOf(HostTab.Chat) }

    LaunchedEffect(Unit) {
        SessionStore.attach(context)
        while (true) {
            withContext(Dispatchers.IO) { SessionStore.refresh() }
            delay(250)
        }
    }

    MaterialTheme {
        Scaffold(
            bottomBar = {
                NavigationBar {
                    for (item in HostTab.entries) {
                        NavigationBarItem(
                            selected = tab == item,
                            onClick = { tab = item },
                            icon = { Text(item.label.take(1)) },
                            label = { Text(item.label) },
                        )
                    }
                }
            },
        ) { padding ->
            Box(Modifier.fillMaxSize().padding(padding)) {
                when (tab) {
                    HostTab.Chat -> ChatScreen(state)
                    HostTab.Skills -> SkillsScreen()
                    HostTab.Services -> McpScreen()
                    HostTab.Memory -> MemoryScreen()
                    HostTab.Settings -> SettingsScreen()
                }
            }
        }
    }
}

/** `null` for empty or invalid text, so screens can use `?.let`. */
internal fun parseObject(text: String?): JSONObject? =
    text?.takeIf { it.isNotBlank() }?.let { runCatching { JSONObject(it) }.getOrNull() }
