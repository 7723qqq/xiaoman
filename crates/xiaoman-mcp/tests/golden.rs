//! The MCP fixtures are shared with the Kotlin `GoldenTest`: the same config and env produce the
//! same warnings, and the same text redacts to the same `***` output.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use xiaoman_mcp::{McpConfig, SecretRedactor};

#[test]
fn the_shared_config_redacts_to_the_same_text() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mcp/redaction.json");
    let root: Value =
        serde_json::from_str(&fs::read_to_string(path).expect("fixture")).expect("JSON");
    let env: BTreeMap<String, String> = root["env"]
        .as_object()
        .expect("env")
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().expect("string").to_string()))
        .collect();
    let config = McpConfig::parse(&root["config"].to_string(), &env);
    let warnings: Vec<&str> = root["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .map(|value| value.as_str().expect("string"))
        .collect();
    assert_eq!(warnings, config.warnings);
    let redactor = SecretRedactor::new(&config.servers);
    let texts = root["texts"].as_array().expect("texts");
    assert!(texts.len() >= 5, "expected the shared redaction cases");
    for case in texts {
        let input = case["input"].as_str().expect("input");
        let output = case["output"].as_str().expect("output");
        assert_eq!(output, redactor.redact(input), "{input}");
    }
}
