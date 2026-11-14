use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};
use xiaoman_mcp::{
    McpHttpTransport, McpServerConfig, McpStdioTransport, McpTransport, STDIO_ENVIRONMENT,
};

struct Sequence(Mutex<VecDeque<ResponseTemplate>>);

impl Respond for Sequence {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        self.0
            .lock()
            .unwrap()
            .pop_front()
            .expect("no more responses")
    }
}

async fn server_with(responses: Vec<ResponseTemplate>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(Sequence(Mutex::new(responses.into())))
        .mount(&server)
        .await;
    server
}

fn config(server: &MockServer) -> McpServerConfig {
    let mut config = McpServerConfig::new("maps");
    config.url = Some(format!("{}/mcp", server.uri()));
    config.timeout_seconds = 5;
    config
}

#[tokio::test]
async fn posts_json_and_returns_the_matching_response() {
    let server =
        server_with(vec![ResponseTemplate::new(200).set_body_string(
            r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#,
        )])
        .await;
    let transport = McpHttpTransport::new(config(&server));

    let response = transport
        .exchange(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .await
        .unwrap()
        .expect("response");
    assert_eq!(response["result"]["tools"], json!([]));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["method"], "tools/list");
}

#[tokio::test]
async fn replies_are_matched_by_id_and_notifications_return_none() {
    let server = server_with(vec![
        ResponseTemplate::new(200).set_body_string(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
        ResponseTemplate::new(200).set_body_string(""),
    ])
    .await;
    let transport = McpHttpTransport::new(config(&server));

    assert!(
        transport
            .exchange(json!({"jsonrpc":"2.0","id":1,"method":"x"}))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        transport
            .exchange(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn session_and_protocol_headers_follow_initialize() {
    let server = server_with(vec![
        ResponseTemplate::new(200)
            .insert_header("Mcp-Session-Id", "session-1")
            .set_body_string(
                r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18"}}"#,
            ),
        ResponseTemplate::new(200).set_body_string(r#"{"jsonrpc":"2.0","id":2,"result":{}}"#),
    ])
    .await;
    let transport = McpHttpTransport::new(config(&server));

    transport
        .exchange(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
        .await
        .unwrap();
    transport
        .exchange(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("mcp-session-id").is_none());
    assert!(requests[0].headers.get("mcp-protocol-version").is_none());
    assert_eq!(
        requests[1].headers.get("mcp-session-id").unwrap(),
        "session-1"
    );
    assert_eq!(
        requests[1].headers.get("mcp-protocol-version").unwrap(),
        "2025-06-18"
    );
}

#[tokio::test]
async fn parses_server_sent_events() {
    let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":true}}\n\n";
    let server = server_with(vec![
        ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "text/event-stream"),
    ])
    .await;
    let transport = McpHttpTransport::new(config(&server));

    let response = transport
        .exchange(json!({"jsonrpc":"2.0","id":7,"method":"tools/call"}))
        .await
        .unwrap()
        .expect("response");
    assert_eq!(response["result"]["ok"], json!(true));
}

#[tokio::test]
async fn stdio_environment_allowlist_hides_other_variables() {
    let script = r#"
import json, os, sys
for line in sys.stdin:
    message = json.loads(line)
    seen = ','.join(sorted(k for k in ('PATH', 'HOME', 'MCP_TEST_VALUE') if k in os.environ))
    print(json.dumps({'jsonrpc':'2.0','id':message['id'],'result':{'content':[{'type':'text','text':seen}]}}), flush=True)
"#;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("server.py");
    std::fs::write(&path, script).unwrap();
    let mut config = McpServerConfig::new("local");
    config.command = Some(if cfg!(windows) { "python" } else { "python3" }.to_string());
    config.args = vec!["-u".to_string(), path.to_string_lossy().into_owned()];
    config
        .env
        .insert("MCP_TEST_VALUE".into(), "configured".into());
    config.timeout_seconds = 10;
    let transport = McpStdioTransport::spawn(config, Some(&["PATH"])).unwrap();
    let response = transport
        .exchange(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}))
        .await
        .unwrap()
        .expect("response");
    assert_eq!(
        response["result"]["content"][0]["text"],
        "MCP_TEST_VALUE,PATH"
    );
    transport.close();
    assert!(STDIO_ENVIRONMENT.contains(&"PATH"));
}

#[tokio::test]
async fn http_errors_hide_the_server_url() {
    let server = MockServer::start().await;
    let url = format!("{}/mcp", server.uri());
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_string(format!(r#"{{"error":"POST {url} failed"}}"#)),
        )
        .mount(&server)
        .await;
    let mut config = McpServerConfig::new("maps");
    config.url = Some(url.clone());
    let transport = McpHttpTransport::new(config);

    let error = transport
        .exchange(json!({"jsonrpc":"2.0","id":1,"method":"x"}))
        .await
        .unwrap_err();
    assert!(error.0.contains("HTTP 500"), "{error}");
    assert!(error.0.contains("[服务器地址]"), "{error}");
    assert!(!error.0.contains(&url), "{error}");
}
