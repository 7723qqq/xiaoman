//! JSON views of the session for front ends that are not Rust, today the Android shell's JNI
//! layer.
//!
//! The shapes are deliberately flat and stable: the Kotlin side renders rows and forms from them
//! and never parses Rust types. Every function is pure, so the mapping is covered by the crate's
//! tests rather than by the platform that consumes it.

use serde_json::{Map, Value, json};
use xiaoman_api::{Note, SearchHit, Skill, SkillLoadResult};
use xiaoman_mcp::{
    CatalogKey, CatalogServer, ConfirmPolicy, CustomMcpServer, McpPlatform, McpSelection,
    ServerStatus,
};

use crate::chat_item::ChatItem;
use crate::session::SessionSnapshot;
use crate::settings::{AppSettings, ModelSettings};

/// One chat row: `kind` is `user`, `assistant`, `progress`, `question` or `error`.
pub fn chat_item(item: &ChatItem) -> Value {
    match item {
        ChatItem::User { id, text } => json!({"id": id, "kind": "user", "text": text}),
        ChatItem::Assistant { id, text } => json!({"id": id, "kind": "assistant", "text": text}),
        ChatItem::Progress { id, text, ok } => {
            json!({"id": id, "kind": "progress", "text": text, "ok": ok})
        }
        ChatItem::Question { id, text, takeover } => {
            json!({"id": id, "kind": "question", "text": text, "takeover": takeover})
        }
        ChatItem::Error { id, text } => json!({"id": id, "kind": "error", "text": text}),
    }
}

/// Everything the chat screen reads at one moment.
pub fn snapshot(snapshot: &SessionSnapshot) -> Value {
    json!({
        "items": snapshot.items.iter().map(chat_item).collect::<Vec<_>>(),
        "busy": snapshot.busy,
        "status": snapshot.status,
        "configured": snapshot.configured,
        "question": snapshot.question.as_ref().map(|question| json!({
            "id": question.id,
            "taskId": question.task_id,
            "text": question.text,
            "takeover": question.takeover,
        })),
        "approval": snapshot.approval.as_ref().map(|approval| json!({
            "id": approval.id,
            "taskId": approval.task_id,
            "prompt": approval.prompt,
            "reason": approval.reason,
        })),
    })
}

// ---------------------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------------------

/// The two endpoints and the agent knobs, camelCase, API keys included: the settings screen shows
/// and edits them. Nothing here logs the value.
pub fn settings(settings: &AppSettings) -> Value {
    json!({
        "chat": model(&settings.chat),
        "operator": model(&settings.operator),
        "maxSteps": settings.max_steps,
        "learnFromSuccess": settings.learn_from_success,
    })
}

fn model(model: &ModelSettings) -> Value {
    json!({
        "baseUrl": model.base_url,
        "model": model.model,
        "apiKey": model.api_key,
        "temperature": model.temperature,
    })
}

/// Parses what [`settings`] writes. A missing `apiKey` keeps the stored one? No: the settings
/// screen always sends both keys, so an absent one is an empty key.
pub fn settings_from_value(value: &Value) -> Result<AppSettings, String> {
    let chat = value
        .get("chat")
        .ok_or_else(|| "settings.chat is missing".to_string())?;
    let operator = value
        .get("operator")
        .ok_or_else(|| "settings.operator is missing".to_string())?;
    Ok(AppSettings {
        chat: model_from_value(chat)?,
        operator: model_from_value(operator)?,
        max_steps: value
            .get("maxSteps")
            .and_then(Value::as_i64)
            .map(|steps| steps as i32)
            .unwrap_or_else(|| AppSettings::default().max_steps),
        learn_from_success: value
            .get("learnFromSuccess")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| AppSettings::default().learn_from_success),
    })
}

fn model_from_value(value: &Value) -> Result<ModelSettings, String> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    Ok(ModelSettings {
        base_url: text("baseUrl").unwrap_or_else(|| ModelSettings::DEFAULT_BASE_URL.to_string()),
        model: text("model").unwrap_or_default(),
        api_key: text("apiKey").unwrap_or_default(),
        temperature: value.get("temperature").and_then(Value::as_f64),
    })
}

// ---------------------------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------------------------

pub fn skill(skill: &Skill) -> Value {
    json!({
        "name": skill.name,
        "description": skill.description,
        "apps": skill.apps,
        "channel": skill.channel.wire(),
        "confirmBefore": skill.confirm_before,
        "memory": skill.memory_refs,
        "deeplinks": skill.deeplinks.iter().map(|(key, uri)| json!({"key": key, "uri": uri})).collect::<Vec<_>>(),
        "instructions": skill.instructions,
        "source": skill.source,
        "flow": skill.flow.as_ref().map(|flow| json!({
            "steps": flow.steps.iter().map(flow_step).collect::<Vec<_>>(),
        })),
    })
}

fn flow_step(step: &xiaoman_api::FlowStep) -> Value {
    use xiaoman_api::FlowStep;
    match step {
        FlowStep::Launch { package_name } => json!({"kind": "launch", "packageName": package_name}),
        FlowStep::Deeplink { key_or_uri } => json!({"kind": "deeplink", "target": key_or_uri}),
        FlowStep::Ai { goal } => json!({"kind": "ai", "goal": goal}),
        FlowStep::Confirm { question } => json!({"kind": "confirm", "question": question}),
        FlowStep::Tap { target } => json!({"kind": "tap", "target": target}),
        FlowStep::Type { target, text } => json!({"kind": "type", "target": target, "text": text}),
        FlowStep::Wait { millis } => json!({"kind": "wait", "millis": millis}),
        FlowStep::Assert { expectation } => json!({"kind": "assert", "expectation": expectation}),
    }
}

pub fn skill_load(result: &SkillLoadResult) -> Value {
    json!({
        "skills": result.skills.iter().map(skill).collect::<Vec<_>>(),
        "problems": result.problems.iter().map(|problem| json!({
            "source": problem.source,
            "message": problem.message,
            "line": problem.line,
        })).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------------------------

pub fn note(note: &Note) -> Value {
    serde_json::to_value(note).unwrap_or(Value::Null)
}

pub fn search_hits(hits: &[SearchHit]) -> Value {
    Value::Array(
        hits.iter()
            .map(|hit| serde_json::to_value(hit).unwrap_or(Value::Null))
            .collect(),
    )
}

// ---------------------------------------------------------------------------------------------
// MCP
// ---------------------------------------------------------------------------------------------

/// The catalog entries this platform can run, plus what the user ticked.
pub fn catalog(
    servers: &[CatalogServer],
    platform: McpPlatform,
    selection: &McpSelection,
) -> Value {
    json!({
        "platform": platform.name(),
        "servers": servers.iter()
            .filter(|server| server.available_on(platform))
            .map(|server| catalog_server(server, selection))
            .collect::<Vec<_>>(),
        "custom": selection.custom.iter().map(custom_server).collect::<Vec<_>>(),
    })
}

fn catalog_server(server: &CatalogServer, selection: &McpSelection) -> Value {
    json!({
        "id": server.id,
        "title": server.title,
        "category": server.category,
        "description": server.description,
        "official": server.official,
        "remote": server.remote(),
        "enabled": selection.enabled.contains(&server.id),
        "keys": server.keys.iter().map(catalog_key).collect::<Vec<_>>(),
        "values": server.keys.iter().map(|key| {
            let value = selection.keys.get(&key.name).cloned().unwrap_or_default();
            json!({"name": key.name, "value": value})
        }).collect::<Vec<_>>(),
    })
}

fn catalog_key(key: &CatalogKey) -> Value {
    json!({"name": key.name, "label": key.label, "help": key.help})
}

fn custom_server(server: &CustomMcpServer) -> Value {
    json!({
        "title": server.title,
        "url": server.url,
        "headers": server.headers,
        "confirm": confirm_name(server.confirm),
    })
}

pub fn server_status(status: &ServerStatus) -> Value {
    json!({
        "name": status.name,
        "title": status.title,
        "connected": status.connected,
        "toolCount": status.tool_count,
        "error": status.error,
    })
}

pub fn server_statuses(statuses: &[ServerStatus]) -> Value {
    Value::Array(statuses.iter().map(server_status).collect())
}

/// The saved selection in the shape [`selection_from_value`] reads.
pub fn selection(selection: &McpSelection) -> Value {
    json!({
        "enabled": selection.enabled.iter().collect::<Vec<_>>(),
        "keys": selection.keys,
        "custom": selection.custom.iter().map(custom_server).collect::<Vec<_>>(),
    })
}

/// Parses `{"enabled": [...], "keys": {...}, "custom": [{"title","url","headers","confirm"}]}`.
pub fn selection_from_value(value: &Value) -> Result<McpSelection, String> {
    let mut selection = McpSelection::default();
    if let Some(enabled) = value.get("enabled").and_then(Value::as_array) {
        for id in enabled {
            let id = id
                .as_str()
                .ok_or_else(|| "enabled must hold strings".to_string())?;
            selection.enabled.insert(id.to_string());
        }
    }
    if let Some(keys) = value.get("keys").and_then(Value::as_object) {
        for (name, key) in keys {
            let key = key
                .as_str()
                .ok_or_else(|| format!("key {name} must be a string"))?;
            if !key.trim().is_empty() {
                selection.keys.insert(name.clone(), key.to_string());
            }
        }
    }
    if let Some(custom) = value.get("custom").and_then(Value::as_array) {
        for server in custom {
            let title = server
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let url = server
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if title.trim().is_empty() || url.trim().is_empty() {
                return Err("custom servers need a title and a url".to_string());
            }
            let mut headers = std::collections::BTreeMap::new();
            if let Some(map) = server.get("headers").and_then(Value::as_object) {
                for (name, header) in map {
                    if let Some(header) = header.as_str() {
                        headers.insert(name.clone(), header.to_string());
                    }
                }
            }
            let confirm = server
                .get("confirm")
                .and_then(Value::as_str)
                .and_then(parse_confirm)
                .unwrap_or(ConfirmPolicy::Writes);
            selection.custom.push(
                CustomMcpServer::new(title.trim(), url.trim())
                    .with_headers(headers)
                    .with_confirm(confirm),
            );
        }
    }
    Ok(selection)
}

fn confirm_name(confirm: ConfirmPolicy) -> &'static str {
    match confirm {
        ConfirmPolicy::Writes => "WRITES",
        ConfirmPolicy::All => "ALL",
        ConfirmPolicy::None => "NONE",
    }
}

fn parse_confirm(text: &str) -> Option<ConfirmPolicy> {
    match text.trim().to_ascii_uppercase().as_str() {
        "WRITES" => Some(ConfirmPolicy::Writes),
        "ALL" => Some(ConfirmPolicy::All),
        "NONE" => Some(ConfirmPolicy::None),
        _ => None,
    }
}

/// A JSON object with `{"ok": true}` or `{"ok": false, "error": "..."}`.
pub fn result_ok() -> Value {
    json!({"ok": true})
}

pub fn result_error(message: impl Into<String>) -> Value {
    json!({"ok": false, "error": message.into()})
}

/// Convenience for the JNI layer: an object with the given keys.
pub fn object(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<Map<_, _>>(),
    )
}
