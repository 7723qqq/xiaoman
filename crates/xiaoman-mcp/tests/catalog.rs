use std::collections::{BTreeMap, BTreeSet};

use xiaoman_mcp::{ConfirmPolicy, CustomMcpServer, McpCatalog, McpPlatform, McpSelection};

fn pairs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[test]
fn built_in_catalog_loads() {
    let catalog = McpCatalog::built_in();
    let ids: Vec<&str> = catalog
        .servers
        .iter()
        .map(|server| server.id.as_str())
        .collect();
    for id in ["amap", "didi", "aihot", "12306"] {
        assert!(ids.contains(&id), "{ids:?}");
    }
    let amap = catalog.server("amap").unwrap();
    assert_eq!(amap.keys.len(), 1);
    assert_eq!(amap.keys[0].name, "AMAP_MAPS_API_KEY");
    assert!(amap.available_on(McpPlatform::Web) && amap.available_on(McpPlatform::Android));
    let train = catalog.server("12306").unwrap();
    assert!(!train.available_on(McpPlatform::Web));
    assert!(train.available_on(McpPlatform::Desktop));
    assert!(
        catalog
            .available_on(McpPlatform::Web)
            .iter()
            .all(|server| server.remote())
    );
}

#[test]
fn selection_becomes_config_with_keys() {
    let catalog = McpCatalog::built_in();
    let selection = McpSelection {
        enabled: ["amap", "aihot", "didi", "12306", "nope"]
            .iter()
            .map(|id| (*id).to_string())
            .collect::<BTreeSet<_>>(),
        keys: pairs(&[("AMAP_MAPS_API_KEY", "amap-secret-123456")]),
        custom: vec![
            CustomMcpServer::new("我的服务", "https://mcp.example.com/mcp")
                .with_headers(pairs(&[("Authorization", "Bearer custom-token-9876")])),
        ],
    };
    assert_eq!(
        catalog.missing_keys(&selection),
        BTreeMap::from([("didi".to_string(), vec!["DIDI_MCP_KEY".to_string()])])
    );

    let config = catalog.to_config(&selection, McpPlatform::Web);
    assert_eq!(
        config
            .servers
            .iter()
            .map(|server| server.name.as_str())
            .collect::<Vec<_>>(),
        vec!["aihot", "amap", "custom-1"]
    );
    assert_eq!(
        config.servers[1].url.as_deref(),
        Some("https://mcp.amap.com/mcp?key=amap-secret-123456")
    );
    assert_eq!(config.servers[2].title, "我的服务");
    assert_eq!(config.servers[2].confirm, ConfirmPolicy::Writes);

    let warnings = config.warnings.join("\n");
    assert!(
        warnings.contains("火车票查询 只能在电脑上使用"),
        "{warnings}"
    );
    assert!(warnings.contains("未知的 MCP 服务 nope"), "{warnings}");
    assert!(
        warnings.contains("滴滴出行 还没有填写 滴滴 MCP Key，暂未启用"),
        "{warnings}"
    );

    let config_text = config.to_string();
    assert!(
        !warnings.contains("amap-secret-123456") && !config_text.contains("amap-secret-123456")
    );
    assert!(!config_text.contains("custom-token-9876"));
    let selection_text = format!("{selection} {:?}", selection);
    assert!(
        !selection_text.contains("custom-token-9876"),
        "{selection_text}"
    );
    assert!(
        !selection_text.contains("amap-secret-123456"),
        "{selection_text}"
    );
}

#[test]
fn custom_servers_are_checked() {
    let catalog = McpCatalog::built_in();
    let selection = McpSelection {
        custom: vec![
            CustomMcpServer::new("明文", "http://mcp.example.com/mcp"),
            CustomMcpServer::new("占位", "https://mcp.example.com/mcp?k=${HOME}"),
            CustomMcpServer::new("坏地址", "not a url"),
            CustomMcpServer::new("本机", "http://localhost:9000/mcp"),
        ],
        ..Default::default()
    };
    let config = catalog.to_config(&selection, McpPlatform::Desktop);
    assert_eq!(
        config
            .servers
            .iter()
            .map(|server| server.name.as_str())
            .collect::<Vec<_>>(),
        vec!["custom-4"]
    );
    assert_eq!(config.warnings.len(), 3, "{:?}", config.warnings);
}

#[test]
fn describe_omits_entry_secrets() {
    let catalog = McpCatalog::built_in();
    let json = McpCatalog::describe(catalog.server("amap").unwrap()).to_string();
    assert!(json.contains("高德地图") && json.contains("AMAP_MAPS_API_KEY"));
    assert!(!json.contains("mcp.amap.com"));
}

#[test]
fn parse_derives_platforms_and_rejects_duplicate_ids() {
    let catalog = McpCatalog::parse(
        r#"{"servers":[
            {"id":"remote","url":"https://example.test/mcp"},
            {"id":"local","command":"npx","args":["-y","server"]},
            {"id":"pinned","url":"https://example.test/mcp","platforms":["web"]}
        ]}"#,
    )
    .unwrap();
    assert!(catalog.server("remote").unwrap().remote());
    assert_eq!(
        catalog.server("remote").unwrap().platforms,
        BTreeSet::from([
            "android".to_string(),
            "desktop".to_string(),
            "web".to_string()
        ])
    );
    assert!(!catalog.server("local").unwrap().remote());
    assert_eq!(
        catalog.server("local").unwrap().platforms,
        BTreeSet::from(["desktop".to_string()])
    );
    assert_eq!(
        catalog.server("pinned").unwrap().platforms,
        BTreeSet::from(["web".to_string()])
    );

    let duplicate = McpCatalog::parse(r#"{"servers":[{"id":"a"},{"id":"a"}]}"#);
    assert_eq!(duplicate.err().as_deref(), Some("duplicate catalog ids"));
    assert!(McpCatalog::parse(r#"{"servers":[{"title":"no id"}]}"#).is_err());
}
