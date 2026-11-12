use std::collections::{BTreeMap, BTreeSet};

use xiaoman_mcp::{ConfirmPolicy, McpConfig, McpServerConfig};

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[test]
fn standard_and_extended_config() {
    let json = r#"{"_comment":"ignored","mcpServers":{
        "maps":{"url":"https://example.test/mcp","title":"地图","headers":{"X-Client":"xiaoman"},
          "confirm":"none","tools":["weather"],"timeoutSeconds":9,"_comment":"ignored"},
        "local":{"command":"python3","args":["server.py"],"env":{"MODE":"local"}},
        "disabled":{"url":"https://example.test","enabled":false,"confirm":"all"},
        "_comment":"ignored"}}"#;
    let config = McpConfig::parse(json, &BTreeMap::new());
    assert_eq!(config.servers.len(), 2);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);

    let maps = &config.servers[0];
    assert_eq!(maps.title, "地图");
    assert_eq!(maps.confirm, ConfirmPolicy::None);
    assert_eq!(
        maps.tools,
        Some(["weather".to_string()].into_iter().collect::<BTreeSet<_>>())
    );
    assert_eq!(maps.timeout_seconds, 9);
    assert_eq!(
        maps.headers.get("X-Client").map(String::as_str),
        Some("xiaoman")
    );

    let local = &config.servers[1];
    assert_eq!(local.command.as_deref(), Some("python3"));
    assert_eq!(local.args, vec!["server.py"]);
    assert_eq!(local.env.get("MODE").map(String::as_str), Some("local"));
    assert_eq!(local.confirm, ConfirmPolicy::Writes);
    assert!(local.enabled);
    assert_eq!(local.timeout_seconds, 30);
    assert!(
        config
            .servers
            .iter()
            .all(|server| server.name != "disabled")
    );
}

#[test]
fn substitution_across_strings_and_nested_values() {
    let json = r#"{"mcpServers":{
      "http":{"url":"https://example.test?key=${TOKEN}","headers":{"Authorization":"Bearer ${TOKEN}"}},
      "stdio":{"command":"${CMD}","args":["${ARG}"],"env":{"KEY":"${TOKEN}"}}}}"#;
    let config = McpConfig::parse(
        json,
        &env(&[
            ("TOKEN", "secret-token"),
            ("CMD", "python3"),
            ("ARG", "server.py"),
        ]),
    );
    assert_eq!(
        config.servers[0].url.as_deref(),
        Some("https://example.test?key=secret-token")
    );
    assert_eq!(
        config.servers[0]
            .headers
            .get("Authorization")
            .map(String::as_str),
        Some("Bearer secret-token")
    );
    assert_eq!(config.servers[1].args, vec!["server.py"]);
    assert_eq!(
        config.servers[1].env.get("KEY").map(String::as_str),
        Some("secret-token")
    );
    assert!(!format!("{config}").contains("secret-token"));
}

#[test]
fn missing_variable_skips_only_that_server_without_secrets() {
    let json = r#"{"mcpServers":{
      "bad":{"url":"https://example.test?key=${KNOWN}&other=${MISSING}","headers":{"X":"${SECOND}"}},
      "ok":{"url":"https://example.test","_comment":"${IGNORED}"}}}"#;
    let config = McpConfig::parse(json, &env(&[("KNOWN", "secret-value")]));
    assert_eq!(
        config
            .servers
            .iter()
            .map(|server| server.name.as_str())
            .collect::<Vec<_>>(),
        vec!["ok"]
    );
    assert_eq!(
        config.warnings,
        vec![
            "服务器 bad 未启用：缺少环境变量 MISSING".to_string(),
            "服务器 bad 未启用：缺少环境变量 SECOND".to_string(),
        ]
    );
    assert!(!format!("{config}").contains("secret-value"));
}

#[test]
fn invalid_server_does_not_disable_others() {
    let json = r#"{"mcpServers":{
      "bad name":{"url":"https://example.test"},
      "both":{"url":"https://example.test","command":"secret-command"},
      "timeout":{"url":"https://example.test","timeoutSeconds":0},
      "ok":{"url":"https://example.test"}}}"#;
    let config = McpConfig::parse(json, &BTreeMap::new());
    assert_eq!(
        config
            .servers
            .iter()
            .map(|server| server.name.as_str())
            .collect::<Vec<_>>(),
        vec!["ok"]
    );
    assert_eq!(config.warnings.len(), 3, "{:?}", config.warnings);
    assert!(!format!("{:?}", config.warnings).contains("secret-command"));
}

#[test]
fn to_string_masks_url_headers_env_and_encoded_secrets() {
    let mut server = McpServerConfig::new("safe");
    server.url = Some("https://example.test?key=url%2Bsecret&extra=another-secret".to_string());
    server.headers.insert(
        "Authorization".to_string(),
        "Bearer header-secret".to_string(),
    );
    server
        .env
        .insert("KEY".to_string(), "env-secret".to_string());
    server.args = vec![
        "url+secret".to_string(),
        "header-secret".to_string(),
        "env-secret".to_string(),
    ];

    let text = server.to_string();
    for secret in [
        "url%2Bsecret",
        "url+secret",
        "another-secret",
        "header-secret",
        "env-secret",
    ] {
        assert!(!text.contains(secret), "{text}");
    }
    assert!(text.contains("key=***"), "{text}");
}

#[test]
fn missing_file_and_invalid_json_are_safe() {
    let directory = tempfile::tempdir().unwrap();
    let missing = McpConfig::load(&directory.path().join("missing.json"), &BTreeMap::new());
    assert!(missing.servers.is_empty());

    let broken = McpConfig::parse("secret-broken-json", &BTreeMap::new());
    assert!(!broken.warnings.is_empty());
    assert!(!format!("{broken}").contains("secret-broken-json"));
}

#[test]
fn disabled_servers_skip_substitution_and_validation_without_warnings() {
    let json = r#"{"mcpServers":{
      "disabled":{"enabled":false,"url":"https://example.test?key=${MISSING}",
        "headers":{"Authorization":"Bearer ${OTHER}"},"env":{"KEY":"${ENV_MISSING}"}},
      "invalid name":{"enabled":false,"command":[],"timeoutSeconds":-1},
      "ok":{"url":"https://example.test"}}}"#;
    let config = McpConfig::parse(json, &BTreeMap::new());
    assert_eq!(
        config
            .servers
            .iter()
            .map(|server| server.name.as_str())
            .collect::<Vec<_>>(),
        vec!["ok"]
    );
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
}

#[test]
fn redaction_preserves_short_values_across_every_secret_source() {
    let json = r#"{"mcpServers":{"maps":{
      "url":"https://example.test?v=1&short=abcde&encoded=%61%62%63%64%65&key=abcdef",
      "headers":{"X-Short":"small","Authorization":"Bearer tiny","X-Key":"header-secret"},
      "env":{"NODE_ENV":"1","KEY":"env-secret"},
      "title":"${SHORT} ${LONG}"}}}"#;
    let config = McpConfig::parse(
        json,
        &env(&[("SHORT", "five!"), ("LONG", "substituted-secret")]),
    );
    let redactor = config.servers.first().unwrap().redactor();

    assert_eq!(
        redactor.redact("温度 18 small tiny abcde %61%62%63%64%65 five!"),
        "温度 18 small tiny abcde %61%62%63%64%65 five!"
    );
    assert_eq!(
        redactor.redact("abcdef header-secret env-secret substituted-secret"),
        "*** *** *** ***"
    );

    let mut bearer = McpServerConfig::new("bearer");
    bearer
        .headers
        .insert("Authorization".to_string(), "Bearer abcdef".to_string());
    assert_eq!(bearer.redactor().redact("abcdef"), "***");
}
