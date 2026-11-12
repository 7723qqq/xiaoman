//! The MCP screen's rows: `McpScreen.kt` over `xiaoman_mcp::McpCatalog` and `McpSelection`.
//!
//! The catalog, the confirm policies and the URL rule live in `xiaoman-mcp`; this module groups
//! the servers by category, tracks the pasted keys and the last test result per server, and keeps
//! the "add a custom server" form in the shapes the screen paints.

use std::collections::{BTreeMap, BTreeSet};

use xiaoman_mcp::{
    CatalogServer, ConfirmPolicy, CustomMcpServer, McpCatalog, McpPlatform, McpSelection,
};

use super::copy;

/// Ticked by 推荐组合: free or easy to obtain, read-only (`McpScreen.kt`).
pub const RECOMMENDED: [&str; 2] = ["amap", "aihot"];

/// One line of the test result under a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusRow {
    Connected { tools: usize, label: String },
    Failed { error: String, label: String },
}

impl StatusRow {
    /// `StatusLine`: `已连接，N 个工具` or `连接失败：…`.
    pub fn from_status(status: &xiaoman_mcp::ServerStatus) -> Self {
        if status.connected {
            Self::Connected {
                tools: status.tool_count,
                label: copy::mcp_connected(status.tool_count),
            }
        } else {
            let error = status.error.clone().unwrap_or_else(|| "?".to_string());
            Self::Failed {
                label: copy::mcp_failed(&error),
                error,
            }
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Connected { label, .. } | Self::Failed { label, .. } => label,
        }
    }

    pub fn ok(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}

/// One key the ticked server needs pasted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRow {
    pub name: String,
    pub label: String,
    pub help: Option<String>,
    pub value: String,
    pub filled: bool,
}

/// One catalog server as the screen paints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRow {
    pub id: String,
    pub title: String,
    /// `官方` or `社区`.
    pub badge: String,
    pub description: String,
    pub enabled: bool,
    pub keys: Vec<KeyRow>,
    /// `还没有填写：…` while a needed key is blank.
    pub missing: Option<String>,
    pub status: Option<StatusRow>,
}

/// A category heading and its servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSection {
    pub category: String,
    pub servers: Vec<ServerRow>,
}

/// A hand-added server already in the selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRow {
    pub title: String,
    /// The URL without its query string, as Compose shows it.
    pub detail: String,
    pub status: Option<StatusRow>,
}

/// The "add a custom server" form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomFormRows {
    pub title_label: String,
    pub url_label: String,
    pub auth_label: String,
    pub confirm_title: String,
    /// `(policy, label, selected)` for the three filter chips.
    pub confirm_options: Vec<(ConfirmPolicy, String, bool)>,
    pub add_label: String,
    pub can_add: bool,
    pub title_value: String,
    pub url_value: String,
    pub auth_value: String,
    /// The URL problem, if the typed address is not an acceptable MCP endpoint.
    pub problem: Option<String>,
}

/// The MCP screen's state.
#[derive(Debug, Clone)]
pub struct McpModel {
    selection: McpSelection,
    servers: Vec<CatalogServer>,
    statuses: BTreeMap<String, StatusRow>,
    testing: bool,
    saved: bool,
    custom_title: String,
    custom_url: String,
    custom_auth: String,
    custom_confirm: ConfirmPolicy,
}

impl McpModel {
    /// The bundled catalog, filtered to the servers this platform can run.
    pub fn bundled(selection: McpSelection, platform: McpPlatform) -> Self {
        Self::with_servers(
            selection,
            McpCatalog::built_in()
                .available_on(platform)
                .into_iter()
                .cloned()
                .collect(),
        )
    }

    /// A catalog in hand, for tests and for the desktop shell's own filter.
    pub fn with_servers(selection: McpSelection, servers: Vec<CatalogServer>) -> Self {
        Self {
            selection,
            servers,
            statuses: BTreeMap::new(),
            testing: false,
            saved: false,
            custom_title: String::new(),
            custom_url: String::new(),
            custom_auth: String::new(),
            custom_confirm: ConfirmPolicy::Writes,
        }
    }

    pub fn selection(&self) -> &McpSelection {
        &self.selection
    }

    pub fn saved(&self) -> bool {
        self.saved
    }

    pub fn testing(&self) -> bool {
        self.testing
    }

    pub fn set_testing(&mut self, testing: bool) {
        self.testing = testing;
        if testing {
            self.statuses.clear();
        }
    }

    pub fn enabled_count(&self) -> usize {
        self.selection.enabled.len() + self.selection.custom.len()
    }

    pub fn enabled_count_label(&self) -> String {
        copy::mcp_enabled_count(self.enabled_count())
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.selection.enabled.contains(id)
    }

    pub fn set_enabled(&mut self, id: &str, on: bool) {
        if on {
            self.selection.enabled.insert(id.to_string());
        } else {
            self.selection.enabled.remove(id);
        }
        self.saved = false;
    }

    pub fn toggle_enabled(&mut self, id: &str) {
        let on = !self.is_enabled(id);
        self.set_enabled(id, on);
    }

    pub fn set_key(&mut self, name: &str, value: &str) {
        self.selection
            .keys
            .insert(name.to_string(), value.trim().to_string());
        self.saved = false;
    }

    /// `推荐组合`: ticks the recommended ids without touching the pasted keys.
    pub fn apply_recommended(&mut self) {
        for id in RECOMMENDED {
            self.selection.enabled.insert(id.to_string());
        }
        self.saved = false;
    }

    /// The test result line for a catalog server.
    pub fn status(&self, id: &str) -> Option<&StatusRow> {
        self.statuses.get(id)
    }

    /// `graph.testMcp(selection)`: every status replaces the previous set.
    pub fn record_statuses(&mut self, statuses: Vec<xiaoman_mcp::ServerStatus>) {
        self.statuses.clear();
        for status in statuses {
            self.statuses
                .insert(status.name.clone(), StatusRow::from_status(&status));
        }
        self.testing = false;
    }

    pub fn sections(&self) -> Vec<ServerSection> {
        let mut sections: Vec<ServerSection> = Vec::new();
        for server in &self.servers {
            let row = self.server_row(server);
            match sections
                .iter_mut()
                .find(|section| section.category == server.category)
            {
                Some(section) => section.servers.push(row),
                None => sections.push(ServerSection {
                    category: server.category.clone(),
                    servers: vec![row],
                }),
            }
        }
        sections
    }

    fn server_row(&self, server: &CatalogServer) -> ServerRow {
        let enabled = self.is_enabled(&server.id);
        let keys: Vec<KeyRow> = server
            .keys
            .iter()
            .map(|key| {
                let value = self
                    .selection
                    .keys
                    .get(&key.name)
                    .cloned()
                    .unwrap_or_default();
                KeyRow {
                    name: key.name.clone(),
                    label: key.label.clone(),
                    help: key.help.clone(),
                    filled: !value.trim().is_empty(),
                    value,
                }
            })
            .collect();
        // A key is only reported missing while the server is ticked, like `ServiceCard`.
        let missing: Vec<String> = if enabled {
            server
                .keys
                .iter()
                .filter(|key| {
                    self.selection
                        .keys
                        .get(&key.name)
                        .is_none_or(|value| value.trim().is_empty())
                })
                .map(|key| key.label.clone())
                .collect()
        } else {
            Vec::new()
        };
        ServerRow {
            id: server.id.clone(),
            title: server.title.clone(),
            badge: if server.official {
                copy::MCP_OFFICIAL
            } else {
                copy::MCP_COMMUNITY
            }
            .to_string(),
            description: server.description.clone(),
            enabled,
            missing: (!missing.is_empty()).then(|| copy::mcp_missing_key(&missing.join("、"))),
            keys,
            status: self.statuses.get(&server.id).cloned(),
        }
    }

    pub fn custom_rows(&self) -> Vec<CustomRow> {
        self.selection
            .custom
            .iter()
            .enumerate()
            .map(|(index, server)| CustomRow {
                title: if server.title.trim().is_empty() {
                    server.url.clone()
                } else {
                    server.title.clone()
                },
                detail: server
                    .url
                    .split('?')
                    .next()
                    .unwrap_or(&server.url)
                    .to_string(),
                status: self.statuses.get(&format!("custom-{}", index + 1)).cloned(),
            })
            .collect()
    }

    pub fn custom_form(&self) -> CustomFormRows {
        let problem = self.url_problem();
        CustomFormRows {
            title_label: copy::MCP_CUSTOM_NAME.to_string(),
            url_label: copy::MCP_CUSTOM_URL.to_string(),
            auth_label: copy::MCP_CUSTOM_AUTH.to_string(),
            confirm_title: copy::MCP_CUSTOM_CONFIRM.to_string(),
            confirm_options: vec![
                (
                    ConfirmPolicy::Writes,
                    copy::MCP_CONFIRM_WRITES.to_string(),
                    self.custom_confirm == ConfirmPolicy::Writes,
                ),
                (
                    ConfirmPolicy::All,
                    copy::MCP_CONFIRM_ALL.to_string(),
                    self.custom_confirm == ConfirmPolicy::All,
                ),
                (
                    ConfirmPolicy::None,
                    copy::MCP_CONFIRM_NONE.to_string(),
                    self.custom_confirm == ConfirmPolicy::None,
                ),
            ],
            add_label: copy::MCP_CUSTOM_ADD.to_string(),
            can_add: self.can_add_custom(),
            title_value: self.custom_title.clone(),
            url_value: self.custom_url.clone(),
            auth_value: self.custom_auth.clone(),
            problem: problem.map(copy::mcp_custom_invalid),
        }
    }

    /// `McpCatalog.validateCustomUrl` for the typed address; `None` for a blank one.
    pub fn url_problem(&self) -> Option<&'static str> {
        if self.custom_url.trim().is_empty() {
            return None;
        }
        McpCatalog::validate_custom_url(&self.custom_url)
    }

    pub fn set_custom_title(&mut self, title: impl Into<String>) {
        self.custom_title = title.into();
    }

    pub fn set_custom_url(&mut self, url: impl Into<String>) {
        self.custom_url = url.into().trim().to_string();
    }

    pub fn set_custom_auth(&mut self, auth: impl Into<String>) {
        self.custom_auth = auth.into();
    }

    pub fn set_custom_confirm(&mut self, confirm: ConfirmPolicy) {
        self.custom_confirm = confirm;
    }

    pub fn custom_confirm(&self) -> ConfirmPolicy {
        self.custom_confirm
    }

    pub fn can_add_custom(&self) -> bool {
        !self.custom_url.trim().is_empty() && self.url_problem().is_none()
    }

    /// The Add chip: appends the typed server, then clears the form back to `WRITES`.
    pub fn add_custom(&mut self) -> bool {
        if !self.can_add_custom() {
            return false;
        }
        let mut headers = BTreeMap::new();
        let auth = self.custom_auth.trim();
        if !auth.is_empty() {
            headers.insert("Authorization".to_string(), auth.to_string());
        }
        self.selection.custom.push(CustomMcpServer {
            title: self.custom_title.trim().to_string(),
            url: self.custom_url.clone(),
            headers,
            confirm: self.custom_confirm,
        });
        self.custom_title.clear();
        self.custom_url.clear();
        self.custom_auth.clear();
        self.custom_confirm = ConfirmPolicy::Writes;
        self.saved = false;
        true
    }

    pub fn remove_custom(&mut self, index: usize) {
        if index < self.selection.custom.len() {
            self.selection.custom.remove(index);
            self.saved = false;
        }
    }

    /// Save: the selection the store writes, and the "已保存" note turns on.
    pub fn save(&mut self) -> McpSelection {
        self.saved = true;
        self.selection.clone()
    }

    /// The catalog ids this model lists, for a test or a health check.
    pub fn server_ids(&self) -> BTreeSet<&str> {
        self.servers
            .iter()
            .map(|server| server.id.as_str())
            .collect()
    }
}
