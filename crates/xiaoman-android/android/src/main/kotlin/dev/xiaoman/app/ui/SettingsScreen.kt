package dev.xiaoman.app.ui

import android.Manifest
import android.content.Context
import android.content.Intent
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import dev.xiaoman.app.AgentService
import dev.xiaoman.app.RustSession
import dev.xiaoman.app.accessibilityEnabled
import dev.xiaoman.app.notificationsAllowed
import dev.xiaoman.hands.NativeLibrary
import org.json.JSONObject

private data class ModelForm(
    val baseUrl: String = "https://ark.cn-beijing.volces.com/api/v3",
    val model: String = "",
    val apiKey: String = "",
    val temperature: String = "",
)

private data class SettingsForm(
    val chat: ModelForm = ModelForm(),
    val operator: ModelForm = ModelForm(),
    val maxSteps: String = "30",
    val learn: Boolean = true,
)

/**
 * Settings: the two OpenAI-compatible endpoints, the step cap, the learning toggle, and the host's
 * switches (accessibility, foreground service, library) in one place.
 */
@Composable
fun SettingsScreen() {
    val context = LocalContext.current
    var form by remember { mutableStateOf<SettingsForm?>(null) }
    var message by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(Unit) {
        form = load()
    }

    val current = form ?: return
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text("模型", style = MaterialTheme.typography.titleMedium)
        Text(
            "两个模型都填 OpenAI 兼容接口；操作模型需要能看图。Key 只保存在本机。",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        ModelSection("聊天模型", current.chat) { form = current.copy(chat = it) }
        ModelSection("操作模型（看图）", current.operator) { form = current.copy(operator = it) }
        OutlinedTextField(
            value = current.maxSteps,
            onValueChange = { form = current.copy(maxSteps = it.filter(Char::isDigit)) },
            label = { Text("每次任务最多步数（1-200）") },
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
        )
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Column(Modifier.weight(1f)) {
                Text("成功后写经验笔记", style = MaterialTheme.typography.bodyMedium)
                Text(
                    "任务成功时把路径写进记忆，下次执行时参考。",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Switch(checked = current.learn, onCheckedChange = { form = current.copy(learn = it) })
        }
        Button(
            onClick = {
                message = if (save(current)) "已保存，下一次对话生效。" else "保存失败：请检查两个模型的 Base URL、模型名和 Key。"
            },
            modifier = Modifier.fillMaxWidth(),
        ) { Text("保存") }
        message?.let { Text(it, style = MaterialTheme.typography.bodySmall) }

        HorizontalDivider(Modifier.padding(vertical = 8.dp))
        Text("宿主", style = MaterialTheme.typography.titleMedium)
        HostStatus(context)

        HorizontalDivider(Modifier.padding(vertical = 8.dp))
        Text("数据", style = MaterialTheme.typography.titleMedium)
        val dataDir = RustSession.nativeDataDir()
        val traces = RustSession.nativeTracesDir()
        Text("数据目录：${dataDir ?: "-"}", style = MaterialTheme.typography.bodySmall)
        Text("轨迹目录：${traces ?: "-"}", style = MaterialTheme.typography.bodySmall)
        Text(
            "技能放在数据目录的 skills/ 下，轨迹在 traces/<run_id>/，记忆在 memory/。",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun ModelSection(title: String, form: ModelForm, onForm: (ModelForm) -> Unit) {
    Text(title, style = MaterialTheme.typography.labelLarge)
    OutlinedTextField(
        value = form.baseUrl,
        onValueChange = { onForm(form.copy(baseUrl = it)) },
        label = { Text("Base URL") },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
    )
    OutlinedTextField(
        value = form.model,
        onValueChange = { onForm(form.copy(model = it)) },
        label = { Text("模型 / 接入点 ID") },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
    )
    OutlinedTextField(
        value = form.apiKey,
        onValueChange = { onForm(form.copy(apiKey = it)) },
        label = { Text("API Key") },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
        visualTransformation = PasswordVisualTransformation(),
    )
    OutlinedTextField(
        value = form.temperature,
        onValueChange = { onForm(form.copy(temperature = it)) },
        label = { Text("temperature（可空，0-2）") },
        modifier = Modifier.fillMaxWidth(),
        singleLine = true,
    )
}

@Composable
private fun HostStatus(context: Context) {
    val accessibility = accessibilityEnabled(context)
    val service = AgentService.isRunning()
    val library = NativeLibrary.loaded
    val canNotify = notificationsAllowed(context)
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { }
    StatusRow("无障碍服务", if (accessibility) "已开启" else "未开启")
    StatusRow("前台服务", if (service) "已开启" else "未开启")
    StatusRow("Rust 库", if (library) "已加载" else "未加载")
    StatusRow("通知权限", if (canNotify) "已允许" else "未允许")
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedButton(onClick = {
            context.startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
        }) { Text("去开启无障碍服务") }
        OutlinedButton(onClick = { AgentService.start(context) }) { Text("启动前台服务") }
    }
    if (!canNotify) {
        OutlinedButton(onClick = { permission.launch(Manifest.permission.POST_NOTIFICATIONS) }) {
            Text("允许通知")
        }
    }
    if (!library) {
        Text("libxiaoman_android.so 未加载，任务无法运行。", color = MaterialTheme.colorScheme.error)
    }
}

@Composable
private fun StatusRow(label: String, value: String) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
        Text(label, style = MaterialTheme.typography.bodyMedium)
        Text(value, style = MaterialTheme.typography.bodyMedium)
    }
}

private fun load(): SettingsForm {
    val root = parseObject(RustSession.nativeSettings()) ?: return SettingsForm()
    fun model(value: JSONObject?): ModelForm = ModelForm(
        baseUrl = value?.optString("baseUrl")?.ifBlank { "https://ark.cn-beijing.volces.com/api/v3" }
            ?: "https://ark.cn-beijing.volces.com/api/v3",
        model = value?.optString("model").orEmpty(),
        apiKey = value?.optString("apiKey").orEmpty(),
        temperature = value?.optDouble("temperature")?.takeIf { !it.isNaN() }?.toString().orEmpty(),
    )
    return SettingsForm(
        chat = model(root.optJSONObject("chat")),
        operator = model(root.optJSONObject("operator")),
        maxSteps = root.optInt("maxSteps", 30).toString(),
        learn = root.optBoolean("learnFromSuccess", true),
    )
}

private fun save(form: SettingsForm): Boolean {
    val root = JSONObject()
        .put("chat", json(form.chat))
        .put("operator", json(form.operator))
        .put("maxSteps", form.maxSteps.toIntOrNull()?.coerceIn(1, 200) ?: 30)
        .put("learnFromSuccess", form.learn)
    return RustSession.nativeSaveSettings(root.toString())
}

private fun json(form: ModelForm): JSONObject {
    val value = JSONObject()
        .put("baseUrl", form.baseUrl.trim())
        .put("model", form.model.trim())
        .put("apiKey", form.apiKey.trim())
    form.temperature.trim().toDoubleOrNull()?.let { value.put("temperature", it) }
    return value
}
