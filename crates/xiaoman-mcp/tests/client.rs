use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use xiaoman_mcp::{McpClient, McpError, McpServerConfig, McpTransport};

struct FakeTransport {
    responses: Mutex<VecDeque<Option<Value>>>,
    requests: Mutex<Vec<Value>>,
}

impl FakeTransport {
    fn new(responses: Vec<Option<Value>>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl McpTransport for FakeTransport {
    async fn exchange(&self, message: Value) -> Result<Option<Value>, McpError> {
        self.requests.lock().unwrap().push(message.clone());
        if message.get("id").is_none() {
            // Notifications are fire-and-forget, exactly like the real transports.
            return Ok(None);
        }
        Ok(self.responses.lock().unwrap().pop_front().flatten())
    }

    fn close(&self) {}
}

fn client(transport: Arc<FakeTransport>) -> McpClient {
    McpClient::new(McpServerConfig::new("maps"), transport)
}

fn ok(id: i64, result: Value) -> Option<Value> {
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

#[tokio::test]
async fn initialize_sends_protocol_version_then_the_notification() {
    let transport = FakeTransport::new(vec![ok(1, json!({"protocolVersion": "2025-06-18"}))]);
    client(transport.clone()).initialize().await.unwrap();

    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["method"], "initialize");
    assert_eq!(requests[0]["params"]["protocolVersion"], "2025-06-18");
    assert_eq!(requests[0]["params"]["clientInfo"]["name"], "xiaoman");
    assert_eq!(requests[1]["method"], "notifications/initialized");
    assert!(requests[1].get("id").is_none());
}

#[tokio::test]
async fn list_tools_follows_pagination() {
    let transport = FakeTransport::new(vec![
        ok(1, json!({})),
        ok(
            2,
            json!({"tools": [{"name": "weather", "description": "天气", "inputSchema": {"type": "object"}}], "nextCursor": "page-2"}),
        ),
        ok(
            3,
            json!({"tools": [{"name": "route", "annotations": {"title": "路线", "readOnlyHint": true}}]}),
        ),
    ]);
    let tools = client(transport.clone()).list_tools().await.unwrap();

    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["weather", "route"]
    );
    assert_eq!(tools[0].description.as_deref(), Some("天气"));
    assert_eq!(tools[1].title.as_deref(), Some("路线"));
    assert_eq!(tools[1].read_only, Some(true));
    assert_eq!(
        tools[1].input_schema,
        json!({"type": "object", "properties": {}})
    );

    let requests = transport.requests();
    assert_eq!(requests[3]["params"]["cursor"], "page-2");
}

#[tokio::test]
async fn a_repeated_cursor_is_an_error() {
    let transport = FakeTransport::new(vec![
        ok(1, json!({})),
        ok(2, json!({"tools": [], "nextCursor": "same"})),
        ok(3, json!({"tools": [], "nextCursor": "same"})),
    ]);
    let error = client(transport).list_tools().await.unwrap_err();
    assert!(error.0.contains("分页游标重复"), "{error}");
}

#[tokio::test]
async fn call_tool_decodes_parts_and_errors() {
    let transport = FakeTransport::new(vec![
        ok(1, json!({})),
        ok(
            2,
            json!({"content": [
                {"type": "text", "text": "晴"},
                {"type": "image", "data": "..."},
                {"type": "resource", "resource": {}}
            ]}),
        ),
    ]);
    let result = client(transport)
        .call_tool("weather", json!({"city": "北京"}))
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(result.text, "晴\n[图片内容]\n[资源内容]");
}

#[tokio::test]
async fn call_tool_falls_back_to_structured_content() {
    let transport = FakeTransport::new(vec![
        ok(1, json!({})),
        ok(
            2,
            json!({"isError": true, "content": [], "structuredContent": {"code": 42}}),
        ),
    ]);
    let result = client(transport)
        .call_tool("weather", json!({}))
        .await
        .unwrap();
    assert!(!result.ok);
    assert_eq!(result.text, "{\"code\":42}");
}

#[tokio::test]
async fn rpc_errors_are_reported_with_code_and_message() {
    let transport = FakeTransport::new(vec![Some(
        json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "method not found"}}),
    )]);
    let error = client(transport).initialize().await.unwrap_err();
    assert!(error.0.contains("-32601"), "{error}");
    assert!(error.0.contains("method not found"), "{error}");
}
