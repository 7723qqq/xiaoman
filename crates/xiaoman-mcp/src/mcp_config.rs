//! MCP server configuration: parsing, environment substitution and secret redaction.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap());
static VARIABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)}").unwrap());
static QUERY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([?&][^=&#]+)=([^&#]*)").unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmPolicy {
    Writes,
    All,
    None,
}

#[derive(Clone)]
pub struct McpServerConfig {
    pub name: String,
    pub title: String,
    pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub enabled: bool,
    pub confirm: ConfirmPolicy,
    pub tools: Option<BTreeSet<String>>,
    pub timeout_seconds: i64,
    pub(crate) substituted_secrets: Vec<String>,
}

impl McpServerConfig {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            title: name.clone(),
            name,
            url: None,
            headers: BTreeMap::new(),
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            enabled: true,
            confirm: ConfirmPolicy::Writes,
            tools: None,
            timeout_seconds: 30,
            substituted_secrets: Vec::new(),
        }
    }

    pub fn redactor(&self) -> SecretRedactor {
        SecretRedactor::new(std::slice::from_ref(self))
    }
}

impl std::fmt::Display for McpServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let url = self
            .url
            .as_ref()
            .map(|url| QUERY.replace_all(url, "$1=***").to_string());
        let headers: Vec<String> = self
            .headers
            .keys()
            .map(|key| format!("{key}=***"))
            .collect();
        let env: Vec<String> = self.env.keys().map(|key| format!("{key}=***")).collect();
        let text = format!(
            "McpServerConfig(name={}, title={}, url={url:?}, headers=[{}], command={:?}, args={:?}, env=[{}], enabled={}, confirm={:?}, tools={:?}, timeoutSeconds={})",
            self.name,
            self.title,
            headers.join(", "),
            self.command,
            self.args,
            env.join(", "),
            self.enabled,
            self.confirm,
            self.tools,
            self.timeout_seconds
        );
        f.write_str(&self.redactor().redact(&text))
    }
}

#[derive(Clone, Default)]
pub struct McpConfig {
    pub servers: Vec<McpServerConfig>,
    pub warnings: Vec<String>,
}

impl McpConfig {
    pub fn load(path: &std::path::Path, env: &BTreeMap<String, String>) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text, env),
            Err(_) => Self::default(),
        }
    }

    pub fn parse(json: &str, env: &BTreeMap<String, String>) -> Self {
        let Ok(Value::Object(root)) = serde_json::from_str::<Value>(json) else {
            return Self {
                servers: Vec::new(),
                warnings: vec!["MCP 配置无效：需要 JSON 对象".to_string()],
            };
        };
        let Some(Value::Object(entries)) = root.get("mcpServers") else {
            return Self {
                servers: Vec::new(),
                warnings: vec!["MCP 配置无效：缺少 mcpServers 对象".to_string()],
            };
        };

        let mut servers = Vec::new();
        let mut warnings = Vec::new();
        for (name, value) in entries {
            if name.starts_with('_') {
                continue;
            }
            // Disabled entries need neither valid connection fields nor environment variables.
            if value.get("enabled").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            if !NAME.is_match(name) {
                warnings
                    .push("MCP 服务器未启用：名称只能包含字母、数字、下划线或连字符".to_string());
                continue;
            }
            let Value::Object(raw) = value else {
                warnings.push(format!("服务器 {name} 未启用：配置需要 JSON 对象"));
                continue;
            };
            let mut missing: Vec<String> = Vec::new();
            let mut secrets: Vec<String> = Vec::new();
            let filtered: Map<String, Value> = raw
                .iter()
                .filter(|(key, _)| !key.starts_with('_'))
                .map(|(key, value)| {
                    (
                        key.clone(),
                        substitute_value(value, env, &mut missing, &mut secrets),
                    )
                })
                .collect();
            if !missing.is_empty() {
                for variable in missing {
                    warnings.push(format!("服务器 {name} 未启用：缺少环境变量 {variable}"));
                }
                continue;
            }
            match build_server(name, &filtered, secrets) {
                Ok(server) => servers.push(server),
                Err(()) => warnings.push(format!("服务器 {name} 未启用：配置字段无效")),
            }
        }
        let redactor = SecretRedactor::new(&servers);
        let warnings = warnings
            .iter()
            .map(|warning| redactor.redact(warning))
            .collect();
        Self { servers, warnings }
    }
}

fn build_server(
    name: &str,
    fields: &Map<String, Value>,
    secrets: Vec<String>,
) -> Result<McpServerConfig, ()> {
    let string = |key: &str| fields.get(key).and_then(Value::as_str).map(str::to_string);
    let url = string("url");
    let command = string("command");
    if url.is_some() == command.is_some() {
        return Err(());
    }
    if let Some(url) = &url {
        url::Url::parse(url).map_err(|_| ())?;
    }
    if let Some(command) = &command {
        if command.trim().is_empty() {
            return Err(());
        }
    }
    let confirm = match string("confirm")
        .unwrap_or_else(|| "writes".to_string())
        .as_str()
    {
        "writes" => ConfirmPolicy::Writes,
        "all" => ConfirmPolicy::All,
        "none" => ConfirmPolicy::None,
        _ => return Err(()),
    };
    let timeout_seconds = fields
        .get("timeoutSeconds")
        .and_then(Value::as_i64)
        .unwrap_or(30);
    if timeout_seconds <= 0 {
        return Err(());
    }
    let strings = |key: &str| -> Result<BTreeMap<String, String>, ()> {
        match fields.get(key) {
            None | Some(Value::Null) => Ok(BTreeMap::new()),
            Some(Value::Object(map)) => map
                .iter()
                .map(|(key, value)| match value {
                    Value::String(text) => Ok((key.clone(), text.clone())),
                    _ => Err(()),
                })
                .collect(),
            _ => Err(()),
        }
    };
    let array = |key: &str| -> Result<Vec<String>, ()> {
        match fields.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str().map(str::to_string).ok_or(()))
                .collect(),
            _ => Err(()),
        }
    };
    let tools = array("tools")?.into_iter().collect::<BTreeSet<_>>();
    Ok(McpServerConfig {
        name: name.to_string(),
        title: string("title").unwrap_or_else(|| name.to_string()),
        url,
        headers: strings("headers")?,
        command,
        args: array("args")?,
        env: strings("env")?,
        enabled: fields
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        confirm,
        tools: if tools.is_empty() { None } else { Some(tools) },
        timeout_seconds,
        substituted_secrets: secrets,
    })
}

fn substitute_value(
    element: &Value,
    env: &BTreeMap<String, String>,
    missing: &mut Vec<String>,
    secrets: &mut Vec<String>,
) -> Value {
    match element {
        Value::String(text) => {
            let replaced = VARIABLE.replace_all(text, |captures: &regex::Captures<'_>| {
                let variable = &captures[1];
                match env.get(variable) {
                    Some(value) => {
                        if !secrets.iter().any(|secret| secret == value) {
                            secrets.push(value.clone());
                        }
                        value.clone()
                    }
                    None => {
                        if !missing.iter().any(|name| name == variable) {
                            missing.push(variable.to_string());
                        }
                        String::new()
                    }
                }
            });
            Value::String(replaced.to_string())
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| substitute_value(item, env, missing, secrets))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), substitute_value(value, env, missing, secrets)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Redact values of at least six characters before truncating, including encoded variants.
pub struct SecretRedactor {
    secrets: Vec<String>,
}

impl SecretRedactor {
    pub fn new(servers: &[McpServerConfig]) -> Self {
        let mut secrets: Vec<String> = Vec::new();
        let mut add = |value: &str| {
            if !value.is_empty() && !secrets.iter().any(|secret| secret == value) {
                secrets.push(value.to_string());
            }
        };
        for server in servers {
            for value in server.env.values() {
                add(value);
            }
            for value in server.headers.values() {
                add(value);
                if let Some(token) = value
                    .strip_prefix("Bearer ")
                    .or_else(|| value.strip_prefix("bearer "))
                {
                    add(token);
                }
            }
            for secret in &server.substituted_secrets {
                add(secret);
            }
            if let Some(url) = &server.url {
                if let Ok(parsed) = url::Url::parse(url) {
                    let pairs: Vec<String> = parsed
                        .query_pairs()
                        .map(|(_, value)| value.into_owned())
                        .collect();
                    for value in &pairs {
                        add(value);
                    }
                    if let Some(query) = parsed.query() {
                        for (index, parameter) in query.split('&').enumerate() {
                            let decoded_len = pairs
                                .get(index)
                                .map(|value| value.chars().count())
                                .unwrap_or(0);
                            if decoded_len >= 6 {
                                let raw = parameter
                                    .split_once('=')
                                    .map(|(_, value)| value)
                                    .unwrap_or("");
                                add(raw);
                            }
                        }
                    }
                    if let Some(password) = parsed.password() {
                        add(password);
                    }
                }
            }
        }
        let mut expanded: Vec<String> = Vec::new();
        for secret in secrets
            .into_iter()
            .filter(|secret| secret.chars().count() >= 6)
        {
            let escaped = serde_json::to_string(&secret)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            if !expanded.iter().any(|existing| existing == &secret) {
                expanded.push(secret);
            }
            if !expanded.iter().any(|existing| existing == &escaped) {
                expanded.push(escaped);
            }
        }
        expanded.sort_by_key(|secret| std::cmp::Reverse(secret.chars().count()));
        Self { secrets: expanded }
    }

    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            if !secret.is_empty() {
                out = out.replace(secret.as_str(), "***");
            }
        }
        out
    }

    pub fn redact_json(&self, element: &Value) -> Value {
        match element {
            Value::String(text) => Value::String(self.redact(text)),
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| self.redact_json(item)).collect())
            }
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| (self.redact(key), self.redact_json(value)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

impl std::fmt::Debug for McpServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for McpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let servers = self
            .servers
            .iter()
            .map(|server| server.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            f,
            "McpConfig(servers=[{servers}], warnings={:?})",
            self.warnings
        )
    }
}

impl std::fmt::Debug for McpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
