//! The JSON views the Android shell parses: the session snapshot, settings, skills and the MCP
//! catalog/selection. Pure mappings, so they are pinned here rather than on a device.

use serde_json::json;
use xiaoman_api::{Flow, FlowStep, Skill, SkillChannel, SkillLoadResult, SkillProblem};
use xiaoman_app::chat_item::ChatItem;
use xiaoman_app::json;
use xiaoman_app::session::{PendingApproval, PendingQuestion, SessionSnapshot};
use xiaoman_app::settings::{AppSettings, ModelSettings};
use xiaoman_mcp::{
    CatalogKey, CatalogServer, ConfirmPolicy, CustomMcpServer, McpCatalog, McpPlatform,
    McpSelection, ServerStatus,
};

fn snapshot() -> SessionSnapshot {
    SessionSnapshot {
        items: vec![
            ChatItem::User {
                id: 1,
                text: "在吗".to_string(),
            },
            ChatItem::Progress {
                id: 2,
                text: "打开设置".to_string(),
                ok: Some(true),
            },
        ],
        busy: true,
        status: Some("处理中".to_string()),
        question: Some(PendingQuestion {
            id: 3,
            task_id: "task-1".to_string(),
            text: "请问用哪个地址？".to_string(),
            takeover: false,
        }),
        approval: Some(PendingApproval {
            id: 4,
            task_id: "task-1".to_string(),
            prompt: "即将点击「确认支付」，是否继续？".to_string(),
            reason: "confirmation word '支付'".to_string(),
        }),
        skill_draft: None,
        configured: true,
    }
}

#[test]
fn snapshot_keeps_the_row_kinds_and_both_prompts() {
    let value = json::snapshot(&snapshot());
    assert_eq!("在吗", value["items"][0]["text"]);
    assert_eq!("user", value["items"][0]["kind"]);
    assert_eq!("progress", value["items"][1]["kind"]);
    assert_eq!(true, value["items"][1]["ok"]);
    assert_eq!(true, value["busy"]);
    assert_eq!(true, value["configured"]);
    assert_eq!("请问用哪个地址？", value["question"]["text"]);
    assert_eq!(false, value["question"]["takeover"]);
    assert_eq!(
        "即将点击「确认支付」，是否继续？",
        value["approval"]["prompt"]
    );
    assert_eq!(4, value["approval"]["id"]);
}

#[test]
fn snapshot_without_prompts_writes_null() {
    let value = json::snapshot(&SessionSnapshot::default());
    assert!(value["question"].is_null());
    assert!(value["approval"].is_null());
    assert_eq!(0, value["items"].as_array().unwrap().len());
}

#[test]
fn settings_round_trip_through_the_screen_shape() {
    let settings = AppSettings {
        chat: ModelSettings {
            base_url: "https://ark.example/api/v3".to_string(),
            model: "doubao-chat".to_string(),
            api_key: "key-1234".to_string(),
            temperature: Some(0.3),
        },
        operator: ModelSettings {
            model: "doubao-vision".to_string(),
            api_key: "key-5678".to_string(),
            ..ModelSettings::default()
        },
        max_steps: 12,
        learn_from_success: false,
    };
    let value = json::settings(&settings);
    assert_eq!("doubao-chat", value["chat"]["model"]);
    assert_eq!(0.3, value["chat"]["temperature"]);
    assert_eq!(12, value["maxSteps"]);
    assert_eq!(false, value["learnFromSuccess"]);

    assert_eq!(settings, json::settings_from_value(&value).unwrap());
}

#[test]
fn settings_from_value_fills_defaults_and_rejects_a_missing_endpoint() {
    let value =
        json!({"chat": {"model": "m", "apiKey": "k"}, "operator": {"model": "o", "apiKey": "k"}});
    let settings = json::settings_from_value(&value).unwrap();
    assert_eq!(ModelSettings::DEFAULT_BASE_URL, settings.chat.base_url);
    assert_eq!(30, settings.max_steps);
    assert!(settings.learn_from_success);

    assert!(json::settings_from_value(&json!({"operator": {}})).is_err());
}

#[test]
fn a_skill_carries_its_flow_and_metadata() {
    let skill = Skill {
        name: "taxi-home".to_string(),
        description: "打车回家".to_string(),
        apps: vec!["com.example.taxi".to_string()],
        channel: SkillChannel::DeeplinkThenGui,
        confirm_before: vec!["确认呼叫".to_string()],
        memory_refs: vec!["地址/家".to_string()],
        deeplinks: vec![(
            "search".to_string(),
            "app://route?to={destination}".to_string(),
        )],
        instructions: "# 打车\n".to_string(),
        flow: Some(Flow {
            steps: vec![FlowStep::Launch {
                package_name: "com.example.taxi".to_string(),
            }],
        }),
        source: "skills/taxi-home".to_string(),
    };
    let value = json::skill(&skill);
    assert_eq!("taxi-home", value["name"]);
    assert_eq!("deeplink-then-gui", value["channel"]);
    assert_eq!("确认呼叫", value["confirmBefore"][0]);
    assert_eq!("app://route?to={destination}", value["deeplinks"][0]["uri"]);
    assert_eq!("launch", value["flow"]["steps"][0]["kind"]);

    let load = json::skill_load(&SkillLoadResult {
        skills: vec![skill],
        problems: vec![SkillProblem {
            source: "skills/bad".to_string(),
            message: "缺少 name".to_string(),
            line: Some(3),
        }],
    });
    assert_eq!("缺少 name", load["problems"][0]["message"]);
    assert_eq!(3, load["problems"][0]["line"]);
}

#[test]
fn selection_round_trips_and_rejects_incomplete_custom_servers() {
    let value = json!({
        "enabled": ["amap"],
        "keys": {"AMAP_MAPS_API_KEY": "abc", "BLANK": "  "},
        "custom": [{"title": "我的服务", "url": "https://mcp.example/rpc", "headers": {"X-A": "1"}, "confirm": "all"}],
    });
    let selection = json::selection_from_value(&value).unwrap();
    assert!(selection.enabled.contains("amap"));
    assert_eq!(
        Some("abc"),
        selection.keys.get("AMAP_MAPS_API_KEY").map(String::as_str)
    );
    assert!(!selection.keys.contains_key("BLANK"));
    assert_eq!(1, selection.custom.len());
    assert_eq!(ConfirmPolicy::All, selection.custom[0].confirm);
    assert_eq!(
        Some("1"),
        selection.custom[0].headers.get("X-A").map(String::as_str)
    );

    let error =
        json::selection_from_value(&json!({"custom": [{"title": "", "url": "x"}]})).unwrap_err();
    assert!(error.contains("title"), "{error}");
}

#[test]
fn the_catalog_lists_only_what_the_platform_runs() {
    let servers = vec![
        CatalogServer {
            id: "http".to_string(),
            title: "远程".to_string(),
            category: "出行".to_string(),
            description: "远程服务".to_string(),
            official: false,
            entry: serde_json::json!({"url": "https://mcp.example/rpc"})
                .as_object()
                .unwrap()
                .clone(),
            keys: vec![CatalogKey {
                name: "KEY".to_string(),
                label: "密钥".to_string(),
                help: None,
            }],
            platforms: ["desktop", "android", "web"]
                .iter()
                .map(|p| p.to_string())
                .collect(),
        },
        CatalogServer {
            id: "stdio".to_string(),
            title: "本地".to_string(),
            category: "出行".to_string(),
            description: "需要 Node".to_string(),
            official: false,
            entry: serde_json::json!({"command": "npx"})
                .as_object()
                .unwrap()
                .clone(),
            keys: Vec::new(),
            platforms: ["desktop"].iter().map(|p| p.to_string()).collect(),
        },
    ];
    let selection = McpSelection {
        enabled: ["http".to_string()].into_iter().collect(),
        keys: [("KEY".to_string(), "abc".to_string())]
            .into_iter()
            .collect(),
        custom: vec![CustomMcpServer::new("我的", "https://mcp.example/rpc")],
    };
    let value = json::catalog(&servers, McpPlatform::Android, &selection);
    let ids: Vec<&str> = value["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|server| server["id"].as_str().unwrap())
        .collect();
    assert_eq!(vec!["http"], ids, "the stdio server is desktop-only");
    assert_eq!(true, value["servers"][0]["enabled"]);
    assert_eq!("abc", value["servers"][0]["values"][0]["value"]);
    assert_eq!(1, value["custom"].as_array().unwrap().len());

    let builtin = McpCatalog::built_in();
    let android = json::catalog(
        &builtin.servers,
        McpPlatform::Android,
        &McpSelection::default(),
    );
    assert!(android["servers"].as_array().unwrap().len() <= builtin.servers.len());
}

#[test]
fn server_status_maps_the_test_result() {
    let value = json::server_statuses(&[ServerStatus {
        name: "amap".to_string(),
        title: "高德".to_string(),
        connected: false,
        tool_count: 0,
        error: Some("401".to_string()),
    }]);
    assert_eq!(false, value[0]["connected"]);
    assert_eq!("401", value[0]["error"]);
    assert_eq!("高德", value[0]["title"]);
}
