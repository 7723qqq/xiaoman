//! The ticked MCP services and their persistence.
//!
//! Catalog ids, titles, URLs and confirm policies go to the plain file; API keys and custom
//! request headers only to the secret file shared with [`crate::settings::SettingsStore`]. The
//! stored shape is the Kotlin one: `mcp.enabled` as a list, `mcp.custom` as a list of
//! `{"title", "url", "confirm"}` objects (with the Kotlin enum names `WRITES`/`ALL`/`NONE`),
//! `mcp.keys` as an object of non-blank keys, `mcp.customHeaders` as a list parallel to
//! `mcp.custom`.
//!
//! Nothing that prints a selection shows a secret: [`xiaoman_mcp::McpSelection`] and
//! [`xiaoman_mcp::CustomMcpServer`] already redact values in `Display`/`Debug`, and this store's
//! own `Debug` goes through them.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::Path;

use serde_json::{Map, Value};
use xiaoman_mcp::{ConfirmPolicy, CustomMcpServer, McpSelection};

use crate::settings::{JsonFileStore, KeyValueStore, SharedStore};

/// Plain preference file of [`McpSettingsStore`].
pub const PLAIN_FILE: &str = "mcp.json";

pub const ENABLED_KEY: &str = "mcp.enabled";
pub const CUSTOM_KEY: &str = "mcp.custom";
pub const KEYS_KEY: &str = "mcp.keys";
pub const HEADERS_KEY: &str = "mcp.customHeaders";

pub struct McpSettingsStore {
    plain: Box<dyn KeyValueStore>,
    secret: SharedStore,
    selection: McpSelection,
}

impl McpSettingsStore {
    /// Loads what the stores hold now, like the Kotlin store's constructor.
    pub fn new(plain: Box<dyn KeyValueStore>, secret: SharedStore) -> Self {
        let selection = Self::load(plain.as_ref(), &secret);
        Self {
            plain,
            secret,
            selection,
        }
    }

    /// `mcp.json` inside `dir`; pass [`crate::settings::SettingsStore::secret`] so keys and headers
    /// land in the same secret file as the API keys.
    pub fn open(dir: impl AsRef<Path>, secret: SharedStore) -> io::Result<Self> {
        Ok(Self::new(
            Box::new(JsonFileStore::open(dir.as_ref().join(PLAIN_FILE))?),
            secret,
        ))
    }

    pub fn current(&self) -> &McpSelection {
        &self.selection
    }

    /// Writes the plain file first, then the secret one, then updates [`McpSettingsStore::current`].
    pub fn save(&mut self, selection: McpSelection) -> io::Result<()> {
        let enabled: Vec<Value> = selection.enabled.iter().cloned().map(Value::from).collect();
        self.plain.put(ENABLED_KEY, Value::Array(enabled))?;

        let custom: Vec<Value> = selection
            .custom
            .iter()
            .map(|server| {
                let mut object = Map::new();
                object.insert("title".to_string(), Value::from(server.title.clone()));
                object.insert("url".to_string(), Value::from(server.url.clone()));
                object.insert(
                    "confirm".to_string(),
                    Value::from(confirm_name(server.confirm)),
                );
                Value::Object(object)
            })
            .collect();
        self.plain.put(CUSTOM_KEY, Value::Array(custom))?;

        let keys: Map<String, Value> = selection
            .keys
            .iter()
            .filter(|(_, value)| !value.trim().is_empty())
            .map(|(key, value)| (key.clone(), Value::from(value.clone())))
            .collect();
        self.secret.put(KEYS_KEY, Value::Object(keys))?;

        let headers: Vec<Value> = selection
            .custom
            .iter()
            .map(|server| {
                let object: Map<String, Value> = server
                    .headers
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::from(value.clone())))
                    .collect();
                Value::Object(object)
            })
            .collect();
        self.secret.put(HEADERS_KEY, Value::Array(headers))?;

        self.selection = selection;
        Ok(())
    }

    fn load(plain: &dyn KeyValueStore, secret: &dyn KeyValueStore) -> McpSelection {
        let custom = array(plain.get(CUSTOM_KEY));
        let headers = array(secret.get(HEADERS_KEY));
        McpSelection {
            enabled: plain
                .get(ENABLED_KEY)
                .and_then(|value| value.as_array().cloned())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            keys: plain_keys(secret.get(KEYS_KEY)),
            custom: custom
                .iter()
                .enumerate()
                .filter_map(|(index, element)| {
                    let object = element.as_object()?;
                    let field = |name: &str| object.get(name).and_then(primitive_text);
                    // Without a URL the entry is unusable; the app skips it too.
                    let url = field("url")?;
                    let headers = headers
                        .get(index)
                        .and_then(Value::as_object)
                        .map(|headers| {
                            headers
                                .iter()
                                .map(|(key, value)| {
                                    (key.clone(), primitive_text(value).unwrap_or_default())
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(CustomMcpServer {
                        title: field("title").unwrap_or_default(),
                        url,
                        headers,
                        confirm: field("confirm")
                            .as_deref()
                            .and_then(parse_confirm)
                            .unwrap_or(ConfirmPolicy::Writes),
                    })
                })
                .collect(),
        }
    }
}

impl fmt::Display for McpSettingsStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "McpSettingsStore(selection={})", self.selection)
    }
}

impl fmt::Debug for McpSettingsStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

fn array(value: Option<Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    }
}

fn plain_keys(value: Option<Value>) -> BTreeMap<String, String> {
    match value {
        Some(Value::Object(map)) => map
            .into_iter()
            .map(|(key, value)| (key, primitive_text(&value).unwrap_or_default()))
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// `JsonPrimitive.contentOrNull`: text, number and boolean primitives as text; null and
/// containers have none.
fn primitive_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn confirm_name(confirm: ConfirmPolicy) -> &'static str {
    match confirm {
        ConfirmPolicy::Writes => "WRITES",
        ConfirmPolicy::All => "ALL",
        ConfirmPolicy::None => "NONE",
    }
}

/// Kotlin reads the enum name; the lowercase spellings of the HTTP API load too, so a file
/// written by the web server can be opened here.
fn parse_confirm(text: &str) -> Option<ConfirmPolicy> {
    match text.trim().to_ascii_uppercase().as_str() {
        "WRITES" => Some(ConfirmPolicy::Writes),
        "ALL" => Some(ConfirmPolicy::All),
        "NONE" => Some(ConfirmPolicy::None),
        _ => None,
    }
}
