//! The one-tap MCP catalog bundled with this crate, and the step from a [`McpSelection`] to a
//! [`McpConfig`]. Pasted keys stay in memory: they are substituted into the config, never written
//! back to the catalog.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use serde_json::{Map, Value, json};

use crate::mcp_config::{ConfirmPolicy, McpConfig};

/// `catalog.json`, shared with the Android app and the web version.
pub const CATALOG_JSON: &str = include_str!("../assets/catalog.json");

const ENTRY_FIELDS: [&str; 9] = [
    "title",
    "url",
    "headers",
    "command",
    "args",
    "env",
    "confirm",
    "tools",
    "timeoutSeconds",
];
const ALL_PLATFORMS: [&str; 3] = ["desktop", "android", "web"];
const PLACEHOLDER_ERROR: &str = "地址和请求头里不能包含 ${...}";

/// A key the user pastes when ticking a catalog server; `name` is the `${NAME}` placeholder it fills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogKey {
    pub name: String,
    pub label: String,
    pub help: Option<String>,
}

/// One tickable server of the built-in catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogServer {
    pub id: String,
    pub title: String,
    pub category: String,
    pub description: String,
    pub official: bool,
    /// The `mcpServers` entry this server becomes, still with `${NAME}` placeholders.
    pub entry: Map<String, Value>,
    pub keys: Vec<CatalogKey>,
    /// `desktop` for stdio servers; HTTP servers work everywhere.
    pub platforms: BTreeSet<String>,
}

impl CatalogServer {
    /// True when the entry talks HTTP instead of spawning a process.
    pub fn remote(&self) -> bool {
        self.entry.contains_key("url")
    }

    pub fn available_on(&self, platform: McpPlatform) -> bool {
        self.platforms.contains(platform.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPlatform {
    Desktop,
    Android,
    Web,
}

impl McpPlatform {
    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Android => "android",
            Self::Web => "web",
        }
    }
}

/// A server the user adds by hand: any Streamable HTTP MCP endpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct CustomMcpServer {
    pub title: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub confirm: ConfirmPolicy,
}

impl CustomMcpServer {
    pub fn new(title: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            url: url.into(),
            headers: BTreeMap::new(),
            confirm: ConfirmPolicy::Writes,
        }
    }

    pub fn with_headers(mut self, headers: BTreeMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    pub fn with_confirm(mut self, confirm: ConfirmPolicy) -> Self {
        self.confirm = confirm;
        self
    }
}

impl std::fmt::Display for CustomMcpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let headers: Vec<&str> = self.headers.keys().map(String::as_str).collect();
        write!(
            f,
            "CustomMcpServer(title={}, url=<hidden>, headers=[{}], confirm={:?})",
            self.title,
            headers.join(", "),
            self.confirm
        )
    }
}

impl std::fmt::Debug for CustomMcpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

/// What the user ticked: catalog ids, the keys they pasted (by placeholder name) and their own servers.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct McpSelection {
    pub enabled: BTreeSet<String>,
    pub keys: BTreeMap<String, String>,
    pub custom: Vec<CustomMcpServer>,
}

impl std::fmt::Display for McpSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let enabled: Vec<&str> = self.enabled.iter().map(String::as_str).collect();
        let keys: Vec<&str> = self.keys.keys().map(String::as_str).collect();
        let custom: Vec<String> = self.custom.iter().map(ToString::to_string).collect();
        write!(
            f,
            "McpSelection(enabled=[{}], keys=[{}], custom=[{}])",
            enabled.join(", "),
            keys.join(", "),
            custom.join(", ")
        )
    }
}

impl std::fmt::Debug for McpSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

pub struct McpCatalog {
    pub servers: Vec<CatalogServer>,
}

impl McpCatalog {
    pub fn server(&self, id: &str) -> Option<&CatalogServer> {
        self.servers.iter().find(|server| server.id == id)
    }

    pub fn available_on(&self, platform: McpPlatform) -> Vec<&CatalogServer> {
        self.servers
            .iter()
            .filter(|server| server.available_on(platform))
            .collect()
    }

    /// Keys still missing for the ticked servers, as `id -> key names`.
    pub fn missing_keys(&self, selection: &McpSelection) -> BTreeMap<String, Vec<String>> {
        let mut missing = BTreeMap::new();
        for id in &selection.enabled {
            let Some(server) = self.server(id) else {
                continue;
            };
            let names: Vec<String> = server
                .keys
                .iter()
                .map(|key| key.name.clone())
                .filter(|name| {
                    selection
                        .keys
                        .get(name)
                        .is_none_or(|value| value.trim().is_empty())
                })
                .collect();
            if !names.is_empty() {
                missing.insert(id.clone(), names);
            }
        }
        missing
    }

    /// Ticked catalog servers available on `platform` plus custom ones (named `custom-1`,
    /// `custom-2` ...). A ticked server whose keys are missing is skipped with a warning, as
    /// [`McpConfig::parse`] does.
    pub fn to_config(&self, selection: &McpSelection, platform: McpPlatform) -> McpConfig {
        let mut warnings: Vec<String> = Vec::new();
        let mut servers: Map<String, Value> = Map::new();
        for id in &selection.enabled {
            let Some(server) = self.server(id) else {
                warnings.push(format!("未知的 MCP 服务 {id}，已忽略"));
                continue;
            };
            if !server.available_on(platform) {
                warnings.push(format!("{} 只能在电脑上使用，已跳过", server.title));
                continue;
            }
            let missing: Vec<&CatalogKey> = server
                .keys
                .iter()
                .filter(|key| {
                    selection
                        .keys
                        .get(&key.name)
                        .is_none_or(|value| value.trim().is_empty())
                })
                .collect();
            if !missing.is_empty() {
                let labels: Vec<&str> = missing.iter().map(|key| key.label.as_str()).collect();
                warnings.push(format!(
                    "{} 还没有填写 {}，暂未启用",
                    server.title,
                    labels.join("、")
                ));
                continue;
            }
            servers.insert(id.clone(), Value::Object(server.entry.clone()));
        }
        for (index, custom) in selection.custom.iter().enumerate() {
            let problem = Self::validate_custom_url(&custom.url)
                .map(str::to_string)
                .or_else(|| {
                    std::iter::once(&custom.url)
                        .chain(custom.headers.values())
                        .any(|value| value.contains("${"))
                        .then(|| PLACEHOLDER_ERROR.to_string())
                });
            let Some(problem) = problem else {
                let name = format!("custom-{}", index + 1);
                let mut entry = Map::new();
                let title = if custom.title.trim().is_empty() {
                    format!("自定义服务 {}", index + 1)
                } else {
                    custom.title.clone()
                };
                entry.insert("title".to_string(), Value::String(title));
                entry.insert(
                    "url".to_string(),
                    Value::String(custom.url.trim().to_string()),
                );
                if !custom.headers.is_empty() {
                    let headers: Map<String, Value> = custom
                        .headers
                        .iter()
                        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                        .collect();
                    entry.insert("headers".to_string(), Value::Object(headers));
                }
                entry.insert(
                    "confirm".to_string(),
                    Value::String(confirm_name(custom.confirm).to_string()),
                );
                servers.insert(name, Value::Object(entry));
                continue;
            };
            warnings.push(format!("自定义服务「{}」未启用：{problem}", custom.title));
        }
        let mut root = Map::new();
        root.insert("mcpServers".to_string(), Value::Object(servers));
        let parsed = McpConfig::parse(&Value::Object(root).to_string(), &selection.keys);
        warnings.extend(parsed.warnings);
        McpConfig {
            servers: parsed.servers,
            warnings,
        }
    }

    /// The catalog bundled with this crate.
    pub fn built_in() -> &'static McpCatalog {
        static BUILT_IN: LazyLock<McpCatalog> = LazyLock::new(|| {
            McpCatalog::parse(CATALOG_JSON)
                .unwrap_or_else(|error| panic!("missing assets/catalog.json: {error}"))
        });
        &BUILT_IN
    }

    pub fn parse(text: &str) -> Result<McpCatalog, String> {
        let root: Value =
            serde_json::from_str(text).map_err(|_| "MCP 目录无效：需要 JSON 对象".to_string())?;
        let Value::Object(root) = root else {
            return Err("MCP 目录无效：需要 JSON 对象".to_string());
        };
        let entries = root
            .get("servers")
            .and_then(Value::as_array)
            .ok_or_else(|| "MCP 目录无效：缺少 servers 数组".to_string())?;
        let mut servers = Vec::with_capacity(entries.len());
        for entry in entries {
            let object = entry
                .as_object()
                .ok_or_else(|| "catalog server without id".to_string())?;
            let text_field = |key: &str| object.get(key).and_then(Value::as_str);
            let id = text_field("id")
                .ok_or_else(|| "catalog server without id".to_string())?
                .to_string();
            let title = text_field("title").unwrap_or(&id).to_string();
            let mut keys = Vec::new();
            for item in object
                .get("keys")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let key = item
                    .as_object()
                    .ok_or_else(|| "catalog key without name".to_string())?;
                let name = key
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "catalog key without name".to_string())?
                    .to_string();
                keys.push(CatalogKey {
                    label: key
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or(&name)
                        .to_string(),
                    help: key.get("help").and_then(Value::as_str).map(str::to_string),
                    name,
                });
            }
            let mut fields: Map<String, Value> = object
                .iter()
                .filter(|(key, _)| ENTRY_FIELDS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if !fields.contains_key("title") {
                fields.insert("title".to_string(), Value::String(title.clone()));
            }
            let platforms: BTreeSet<String> =
                match object.get("platforms").and_then(Value::as_array) {
                    Some(items) => items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                    None if fields.contains_key("url") => ALL_PLATFORMS
                        .iter()
                        .map(|platform| (*platform).to_string())
                        .collect(),
                    None => BTreeSet::from(["desktop".to_string()]),
                };
            servers.push(CatalogServer {
                id,
                title,
                category: text_field("category").unwrap_or("其他").to_string(),
                description: text_field("description").unwrap_or_default().to_string(),
                official: matches!(object.get("official"), Some(Value::Bool(true)))
                    || text_field("official") == Some("true"),
                entry: fields,
                keys,
                platforms,
            });
        }
        let unique: BTreeSet<&str> = servers.iter().map(|server| server.id.as_str()).collect();
        if unique.len() != servers.len() {
            return Err("duplicate catalog ids".to_string());
        }
        Ok(McpCatalog { servers })
    }

    /// `None` when `url` is acceptable for a hand-added server: absolute HTTPS (HTTP only for
    /// localhost), and no `${...}` placeholders left for the user to fill.
    pub fn validate_custom_url(url: &str) -> Option<&'static str> {
        let parsed = match url::Url::parse(url.trim()) {
            Ok(parsed) => parsed,
            Err(_) => return Some("地址无效"),
        };
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Some("地址无效");
        }
        let host = parsed.host_str().unwrap_or_default();
        if parsed.scheme() != "https" && host != "localhost" && host != "127.0.0.1" {
            return Some("只支持 https 地址");
        }
        None
    }

    /// For callers that build JSON lists of catalog entries (UI, web API). The entry itself is
    /// omitted: it still carries the `${NAME}` placeholders.
    pub fn describe(server: &CatalogServer) -> Value {
        let platforms: Vec<&str> = server.platforms.iter().map(String::as_str).collect();
        let keys: Vec<Value> = server
            .keys
            .iter()
            .map(|key| {
                let mut object = Map::new();
                object.insert("name".to_string(), Value::String(key.name.clone()));
                object.insert("label".to_string(), Value::String(key.label.clone()));
                if let Some(help) = &key.help {
                    object.insert("help".to_string(), Value::String(help.clone()));
                }
                Value::Object(object)
            })
            .collect();
        json!({
            "id": &server.id,
            "title": &server.title,
            "category": &server.category,
            "description": &server.description,
            "official": server.official,
            "remote": server.remote(),
            "platforms": platforms,
            "keys": keys,
        })
    }
}

fn confirm_name(confirm: ConfirmPolicy) -> &'static str {
    match confirm {
        ConfirmPolicy::Writes => "writes",
        ConfirmPolicy::All => "all",
        ConfirmPolicy::None => "none",
    }
}
