//! Ports of `app/src/test/kotlin/dev/xiaoman/app/settings/McpSettingsStoreTest.kt`, plus the
//! hand-written-file cases the Kotlin loader tolerates.

use std::collections::{BTreeMap, BTreeSet};

use tempfile::TempDir;
use xiaoman_app::mcp_settings::{McpSettingsStore, PLAIN_FILE};
use xiaoman_app::settings::{InMemoryKeyValueStore, SECRET_FILE, SettingsStore, SharedStore};
use xiaoman_mcp::{ConfirmPolicy, CustomMcpServer, McpSelection};

const AMAP_KEY: &str = "amap-key-0123456789";
const BEARER: &str = "Bearer tok-abcdef123";

fn selection() -> McpSelection {
    McpSelection {
        enabled: BTreeSet::from(["amap".to_string(), "aihot".to_string()]),
        keys: BTreeMap::from([
            ("AMAP_MAPS_API_KEY".to_string(), AMAP_KEY.to_string()),
            ("EMPTY".to_string(), " ".to_string()),
        ]),
        custom: vec![CustomMcpServer {
            title: "我的".to_string(),
            url: "https://mcp.example.com/mcp".to_string(),
            headers: BTreeMap::from([("Authorization".to_string(), BEARER.to_string())]),
            confirm: ConfirmPolicy::All,
        }],
    }
}

fn store_in(dir: &TempDir) -> McpSettingsStore {
    let settings = SettingsStore::open(dir.path()).unwrap();
    McpSettingsStore::open(dir.path(), settings.secret()).unwrap()
}

#[test]
fn round_trip_keeps_ticks_keys_and_custom_servers_secrets_only_in_the_secret_file() {
    let dir = TempDir::new().unwrap();
    store_in(&dir).save(selection()).unwrap();

    let loaded = store_in(&dir);
    assert_eq!(loaded.current().enabled, selection().enabled);
    assert_eq!(
        loaded.current().keys,
        BTreeMap::from([("AMAP_MAPS_API_KEY".to_string(), AMAP_KEY.to_string())]),
        "a blank key is not stored"
    );
    assert_eq!(loaded.current().custom, selection().custom);

    let plain = std::fs::read_to_string(dir.path().join(PLAIN_FILE)).unwrap();
    assert!(!plain.contains(AMAP_KEY), "{plain}");
    assert!(!plain.contains("tok-abcdef123"), "{plain}");
    assert!(plain.contains("https://mcp.example.com/mcp"), "{plain}");
    assert!(plain.contains("\"ALL\""), "{plain}");

    let secret = std::fs::read_to_string(dir.path().join(SECRET_FILE)).unwrap();
    assert!(secret.contains(AMAP_KEY), "{secret}");
    assert!(secret.contains(BEARER), "{secret}");
}

#[test]
fn empty_store_loads_an_empty_selection() {
    let dir = TempDir::new().unwrap();
    assert_eq!(store_in(&dir).current(), &McpSelection::default());
}

#[test]
fn nothing_displayable_shows_a_secret() {
    let plain = SharedStore::new(InMemoryKeyValueStore::new());
    let secret = SharedStore::new(InMemoryKeyValueStore::new());
    let mut store = McpSettingsStore::new(Box::new(plain), secret);
    store.save(selection()).unwrap();

    let printed = format!("{store:?}");
    assert!(!printed.contains(AMAP_KEY), "{printed}");
    assert!(!printed.contains("tok-abcdef123"), "{printed}");
    // The placeholder names stay visible: that is what the MCP screen shows.
    assert!(printed.contains("AMAP_MAPS_API_KEY"), "{printed}");
}

#[test]
fn lowercase_confirm_policies_and_broken_entries_load() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(PLAIN_FILE),
        r#"{
            "mcp.enabled": ["amap", 7],
            "mcp.custom": [
                {"title": "小写", "url": "https://a.test/mcp", "confirm": "all"},
                {"title": "未知", "url": "https://b.test/mcp", "confirm": "sometimes"},
                {"title": "没有地址"},
                "not an object"
            ]
        }"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join(SECRET_FILE),
        r#"{"mcp.customHeaders": [{"Authorization": "Bearer x"}, null, {}], "mcp.keys": {"N": 1}}"#,
    )
    .unwrap();

    let loaded = store_in(&dir);
    let selection = loaded.current();
    assert_eq!(selection.enabled, BTreeSet::from(["amap".to_string()]));
    assert_eq!(
        selection.keys,
        BTreeMap::from([("N".to_string(), "1".to_string())])
    );
    assert_eq!(selection.custom.len(), 2);

    assert_eq!(selection.custom[0].confirm, ConfirmPolicy::All);
    assert_eq!(
        selection.custom[0].headers,
        BTreeMap::from([("Authorization".to_string(), "Bearer x".to_string())])
    );

    assert_eq!(
        selection.custom[1].confirm,
        ConfirmPolicy::Writes,
        "unknown policy falls back"
    );
    assert!(selection.custom[1].headers.is_empty());
    assert_eq!(selection.custom[1].title, "未知");
}

#[test]
fn confirm_names_are_the_kotlin_enum_values() {
    let dir = TempDir::new().unwrap();
    let mut store = store_in(&dir);
    for (policy, name) in [
        (ConfirmPolicy::Writes, "WRITES"),
        (ConfirmPolicy::All, "ALL"),
        (ConfirmPolicy::None, "NONE"),
    ] {
        let mut value = selection();
        value.custom[0].confirm = policy;
        store.save(value).unwrap();
        let plain = std::fs::read_to_string(dir.path().join(PLAIN_FILE)).unwrap();
        assert!(plain.contains(&format!("\"{name}\"")), "{plain}");
        assert_eq!(store_in(&dir).current().custom[0].confirm, policy);
    }
}
