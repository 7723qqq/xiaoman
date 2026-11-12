//! Turns the `mcp` object of a request into the servers this turn may call.
//!
//! The catalog step is [`McpCatalog::to_config`] in `xiaoman-mcp`; this module only adds what the
//! page needs on top of it: private endpoints are dropped unless the server allows them, and the
//! "desktop only" warning becomes a note about the missing Node.js runtime.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use xiaoman_mcp::{
    ConfirmPolicy, CustomMcpServer, McpCatalog, McpConfig, McpPlatform, McpSelection,
};

use crate::problem::Problem;
use crate::safe_dns::is_public_url;

#[derive(Debug, Clone, Copy)]
pub struct SelectionOptions {
    pub allow_private: bool,
    pub stdio_mcp: bool,
}

/// Ticked catalog servers plus hand-added ones, filtered like `WebServer.mcpConfig` of the Kotlin
/// server: private or malformed endpoints are dropped with a warning instead of failing the turn.
pub async fn build_mcp_config(
    body: &Value,
    options: SelectionOptions,
    catalog: &McpCatalog,
) -> Result<McpConfig, Problem> {
    let mcp = body
        .get("mcp")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut warnings: Vec<String> = Vec::new();
    let mut selection = McpSelection::default();
    let items = mcp
        .get("custom")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (index, item) in items.iter().enumerate() {
        let Some(object) = item.as_object() else {
            return Err(Problem::bad_request("MCP 配置无效"));
        };
        let url = text(Some(object), "url");
        if !options.allow_private && url::Url::parse(&url).is_ok() && !is_public_url(&url).await {
            warnings.push(format!(
                "自定义服务 {} 未启用：不允许访问内网地址或地址无法解析",
                index + 1
            ));
            continue;
        }
        let confirm = match text(Some(object), "confirm").trim() {
            "" | "writes" => ConfirmPolicy::Writes,
            "all" => ConfirmPolicy::All,
            "none" => ConfirmPolicy::None,
            _ => return Err(Problem::bad_request("MCP 确认策略无效")),
        };
        let headers: BTreeMap<String, String> = object
            .get("headers")
            .and_then(Value::as_object)
            .map(|headers| {
                headers
                    .iter()
                    .filter_map(|(key, value)| Some((key.clone(), primitive_text(value)?)))
                    .collect()
            })
            .unwrap_or_default();
        selection.custom.push(CustomMcpServer {
            title: text(Some(object), "title"),
            url,
            headers,
            confirm,
        });
    }

    selection.enabled = mcp
        .get("enabled")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    selection.keys = mcp
        .get("keys")
        .and_then(Value::as_object)
        .map(|keys| {
            keys.iter()
                .filter_map(|(key, value)| Some((key.clone(), primitive_text(value)?)))
                .collect()
        })
        .unwrap_or_default();

    let platform = if options.stdio_mcp {
        McpPlatform::Desktop
    } else {
        McpPlatform::Web
    };
    let config = catalog.to_config(&selection, platform);

    // Catalog stdio servers only run when this server is allowed to start processes.
    let stdio: BTreeSet<String> = catalog
        .servers
        .iter()
        .filter(|server| !server.remote())
        .map(|server| server.id.clone())
        .collect();
    let servers = config
        .servers
        .into_iter()
        .filter(|server| {
            server.url.is_some() || (options.stdio_mcp && stdio.contains(&server.name))
        })
        .collect();
    let mut mapped: Vec<String> = warnings;
    for warning in config.warnings {
        // `to_config` reports desktop-only servers on the web platform; the page explains the
        // missing Node.js runtime instead.
        let skipped = catalog.servers.iter().find(|server| {
            !server.remote() && warning.starts_with(&format!("{} 只能在电脑上使用", server.title))
        });
        match skipped {
            Some(server) => mapped.push(format!(
                "{} 需要服务器开启 Node.js 服务，当前未开启，已跳过",
                server.title
            )),
            None => mapped.push(warning),
        }
    }
    Ok(McpConfig {
        servers,
        warnings: mapped,
    })
}

fn text(object: Option<&Map<String, Value>>, key: &str) -> String {
    object
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn primitive_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn catalog() -> &'static McpCatalog {
        McpCatalog::built_in()
    }

    fn options(allow_private: bool, stdio: bool) -> SelectionOptions {
        SelectionOptions {
            allow_private,
            stdio_mcp: stdio,
        }
    }

    #[tokio::test]
    async fn private_custom_servers_are_skipped_without_the_flag() {
        let body = json!({ "mcp": { "custom": [
            { "title": "测试服务", "url": "http://127.0.0.1:9000/mcp", "confirm": "writes" },
            { "title": "局域网", "url": "https://10.1.2.3/mcp" },
        ] } });
        let config = build_mcp_config(&body, options(false, false), catalog())
            .await
            .unwrap();
        assert!(config.servers.is_empty());
        assert_eq!(config.warnings.len(), 2);
        assert!(config.warnings[0].contains("不允许访问内网"));

        let config = build_mcp_config(&body, options(true, false), catalog())
            .await
            .unwrap();
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers[0].name, "custom-1");
    }

    #[tokio::test]
    async fn stdio_catalog_servers_need_the_flag() {
        let body = json!({ "mcp": { "enabled": ["12306", "amap"], "keys": { "AMAP_MAPS_API_KEY": "key" } } });
        let config = build_mcp_config(&body, options(true, false), catalog())
            .await
            .unwrap();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].name, "amap");
        assert!(
            config
                .warnings
                .iter()
                .any(|warning| warning.contains("Node.js"))
        );

        let config = build_mcp_config(&body, options(true, true), catalog())
            .await
            .unwrap();
        assert_eq!(config.servers.len(), 2);
    }

    #[tokio::test]
    async fn invalid_confirm_policy_is_a_bad_request() {
        let body = json!({ "mcp": { "custom": [
            { "title": "x", "url": "https://example.com/mcp", "confirm": "sometimes" },
        ] } });
        let problem = build_mcp_config(&body, options(true, false), catalog())
            .await
            .unwrap_err();
        assert_eq!(problem.status, axum::http::StatusCode::BAD_REQUEST);
        assert_eq!(problem.message, "MCP 确认策略无效");
        let body = json!({ "mcp": { "custom": ["not an object"] } });
        assert_eq!(
            build_mcp_config(&body, options(true, false), catalog())
                .await
                .unwrap_err()
                .message,
            "MCP 配置无效"
        );
    }
}
