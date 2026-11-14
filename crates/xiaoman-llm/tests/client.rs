use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};
use xiaoman_api::{
    ChatMessage, ChatRequest, ChatRole, ContentPart, LlmClient, ModelConfig, ModelRole, ToolCall,
    ToolSpec,
};
use xiaoman_llm::{ClientOptions, OpenAiCompatibleClient, SleepFn};

const API_KEY: &str = "sk-secret-123456";
const ENDPOINT: &str = "/api/v3/chat/completions";

struct Sequence {
    responses: Mutex<VecDeque<ResponseTemplate>>,
}

impl Sequence {
    fn new(responses: Vec<ResponseTemplate>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

impl Respond for Sequence {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("no more queued responses")
    }
}

async fn server_with(responses: Vec<ResponseTemplate>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(ENDPOINT))
        .respond_with(Sequence::new(responses))
        .mount(&server)
        .await;
    server
}

fn recorder() -> (SleepFn, Arc<Mutex<Vec<u64>>>) {
    let sleeps = Arc::new(Mutex::new(Vec::new()));
    let sink = sleeps.clone();
    let sleep: SleepFn = Arc::new(move |millis| {
        sink.lock().unwrap().push(millis);
        Box::pin(async {})
    });
    (sleep, sleeps)
}

fn config(server: &MockServer) -> ModelConfig {
    let mut config = ModelConfig::new(
        ModelRole::Chat,
        format!("{}/api/v3/", server.uri()),
        "test-model",
        API_KEY,
    );
    config.timeout_seconds = 10;
    config.extra_headers.insert("X-Extra".into(), "yes".into());
    config
}

fn client(server: &MockServer, max_retries: u32, sleep: SleepFn) -> OpenAiCompatibleClient {
    OpenAiCompatibleClient::with_options(
        config(server),
        ClientOptions {
            max_retries,
            base_delay_millis: 100,
            sleep,
            http: None,
        },
    )
}

fn simple_reply() -> String {
    r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop","logprobs":null}],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4},"extra":1}"#.to_string()
}

async fn bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect()
}

#[tokio::test]
async fn request_body_includes_image_tools_json_mode_and_sampling_params() {
    let server = server_with(vec![
        ResponseTemplate::new(200).set_body_string(simple_reply()),
    ])
    .await;
    let (sleep, _) = recorder();
    let mut config = config(&server);
    config.temperature = Some(0.2);
    config.max_tokens = Some(256);
    let client = OpenAiCompatibleClient::with_options(
        config,
        ClientOptions {
            max_retries: 2,
            base_delay_millis: 100,
            sleep,
            http: None,
        },
    );

    let png = vec![1u8, 2, 3, 4];
    let request = ChatRequest {
        messages: vec![
            ChatMessage::system("be brief"),
            ChatMessage::new(
                ChatRole::User,
                vec![
                    ContentPart::Text("what is this?".into()),
                    ContentPart::ImagePng(png),
                ],
            ),
            ChatMessage {
                role: ChatRole::Assistant,
                parts: vec![],
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "weather".into(),
                    arguments_json: r#"{"city":"北京"}"#.into(),
                }],
                tool_call_id: None,
            },
            ChatMessage::tool("call_1", "sunny"),
        ],
        tools: vec![ToolSpec {
            name: "weather".into(),
            description: "Get the weather".into(),
            parameters: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        }],
        json_mode: true,
    };

    let response = client.complete(request).await.unwrap();
    assert_eq!(response.text, "hi");
    assert_eq!(response.finish_reason.as_deref(), Some("stop"));
    assert_eq!(
        response
            .usage
            .map(|u| (u.prompt_tokens, u.completion_tokens)),
        Some((3, 1))
    );

    let recorded = server.received_requests().await.unwrap();
    let first = &recorded[0];
    assert_eq!(first.url.path(), ENDPOINT);
    assert_eq!(
        first
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        format!("Bearer {API_KEY}")
    );
    assert_eq!(
        first.headers.get("x-extra").unwrap().to_str().unwrap(),
        "yes"
    );

    let body = &bodies(&server).await[0];
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["temperature"], 0.2);
    assert_eq!(body["max_tokens"], 256);
    assert_eq!(body["response_format"]["type"], "json_object");

    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "be brief");

    let user_content = messages[1]["content"].as_array().unwrap();
    assert_eq!(user_content[0]["type"], "text");
    assert_eq!(user_content[0]["text"], "what is this?");
    assert_eq!(user_content[1]["type"], "image_url");
    assert_eq!(
        user_content[1]["image_url"]["url"],
        "data:image/png;base64,AQIDBA=="
    );

    let assistant = &messages[2];
    assert_eq!(assistant["role"], "assistant");
    assert!(assistant["content"].is_null());
    let call = &assistant["tool_calls"][0];
    assert_eq!(call["id"], "call_1");
    assert_eq!(call["type"], "function");
    assert_eq!(call["function"]["name"], "weather");
    assert_eq!(call["function"]["arguments"], r#"{"city":"北京"}"#);

    let tool = &messages[3];
    assert_eq!(tool["role"], "tool");
    assert_eq!(tool["tool_call_id"], "call_1");
    assert_eq!(tool["content"], "sunny");

    let spec = &body["tools"][0];
    assert_eq!(spec["type"], "function");
    assert_eq!(spec["function"]["name"], "weather");
    assert_eq!(spec["function"]["description"], "Get the weather");
    assert_eq!(
        spec["function"]["parameters"],
        json!({"type": "object", "properties": {"city": {"type": "string"}}})
    );
}

#[tokio::test]
async fn text_only_request_uses_plain_string_content_and_omits_optional_fields() {
    let server = server_with(vec![
        ResponseTemplate::new(200).set_body_string(simple_reply()),
    ])
    .await;
    let (sleep, _) = recorder();
    client(&server, 2, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("hello")]))
        .await
        .unwrap();

    let body = &bodies(&server).await[0];
    assert_eq!(body["messages"][0]["content"], "hello");
    assert!(body.get("tools").is_none());
    assert!(body.get("response_format").is_none());
    assert!(body.get("temperature").is_none());
    assert!(body.get("max_tokens").is_none());
}

#[tokio::test]
async fn parses_tool_calls_and_null_content() {
    let reply = r#"{"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[
      {"id":"call_a","type":"function","function":{"name":"open_app","arguments":"{\"pkg\":\"com.x\"}"}},
      {"id":"call_b","type":"function","function":{"name":"noop","arguments":{"k":1}}}
    ]},"finish_reason":"tool_calls"}]}"#;
    let server = server_with(vec![ResponseTemplate::new(200).set_body_string(reply)]).await;
    let (sleep, _) = recorder();
    let response = client(&server, 2, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("go")]))
        .await
        .unwrap();

    assert_eq!(response.text, "");
    assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
    assert!(response.usage.is_none());
    assert_eq!(
        response.tool_calls,
        vec![
            ToolCall {
                id: "call_a".into(),
                name: "open_app".into(),
                arguments_json: r#"{"pkg":"com.x"}"#.into()
            },
            ToolCall {
                id: "call_b".into(),
                name: "noop".into(),
                arguments_json: r#"{"k":1}"#.into()
            },
        ]
    );
}

#[tokio::test]
async fn parses_array_content_of_text_parts() {
    let reply = r#"{"choices":[{"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}]}"#;
    let server = server_with(vec![ResponseTemplate::new(200).set_body_string(reply)]).await;
    let (sleep, _) = recorder();
    let response = client(&server, 2, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("x")]))
        .await
        .unwrap();
    assert_eq!(response.text, "ab");
}

#[tokio::test]
async fn retries_on_429_and_500_with_backoff_and_retry_after() {
    let server = server_with(vec![
        ResponseTemplate::new(429)
            .insert_header("Retry-After", "3")
            .set_body_string(r#"{"error":{"message":"slow down"}}"#),
        ResponseTemplate::new(500).set_body_string("oops"),
        ResponseTemplate::new(200).set_body_string(simple_reply()),
    ])
    .await;
    let (sleep, sleeps) = recorder();
    let response = client(&server, 2, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("x")]))
        .await
        .unwrap();

    assert_eq!(response.text, "hi");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert_eq!(*sleeps.lock().unwrap(), vec![3000, 200]);
}

#[tokio::test]
async fn gives_up_after_max_retries_and_reports_status() {
    let server = server_with(
        (0..3)
            .map(|_| {
                ResponseTemplate::new(503).set_body_string(r#"{"error":{"message":"overloaded"}}"#)
            })
            .collect(),
    )
    .await;
    let (sleep, sleeps) = recorder();
    let error = client(&server, 2, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("x")]))
        .await
        .unwrap_err();

    assert_eq!(error.status_code, Some(503));
    assert!(error.retryable);
    assert!(error.message.contains("overloaded"));
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert_eq!(*sleeps.lock().unwrap(), vec![100, 200]);
}

#[tokio::test]
async fn configured_inference_limit_stops_immediately_without_backoff() {
    let server = server_with(
        (0..3)
            .map(|_| {
                ResponseTemplate::new(429).set_body_string(
                    r#"{"error":{"code":"SetLimitExceeded","message":"inference limit reached"}}"#,
                )
            })
            .collect(),
    )
    .await;
    let (sleep, sleeps) = recorder();
    let error = client(&server, 2, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("x")]))
        .await
        .unwrap_err();

    assert_eq!(error.provider_code.as_deref(), Some("SetLimitExceeded"));
    assert!(!error.retryable);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(sleeps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn keeps_structured_quota_error_code_without_exposing_credentials() {
    let body = format!(
        r#"{{"error":{{"code":"QuotaExceeded.FreeTier","message":"quota exceeded for {API_KEY}"}}}}"#
    );
    let server = server_with(vec![ResponseTemplate::new(429).set_body_string(body)]).await;
    let (sleep, _) = recorder();
    let error = client(&server, 0, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("x")]))
        .await
        .unwrap_err();

    assert_eq!(error.status_code, Some(429));
    assert_eq!(
        error.provider_code.as_deref(),
        Some("QuotaExceeded.FreeTier")
    );
    assert!(!error.message.contains(API_KEY));
}

#[tokio::test]
async fn ignores_a_provider_code_containing_the_api_key() {
    let body = format!(r#"{{"error":{{"code":"{API_KEY}","message":"failed"}}}}"#);
    let server = server_with(vec![ResponseTemplate::new(429).set_body_string(body)]).await;
    let (sleep, _) = recorder();
    let error = client(&server, 0, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("x")]))
        .await
        .unwrap_err();
    assert_eq!(error.provider_code, None);
}

#[tokio::test]
async fn does_not_retry_on_401() {
    let server = server_with(vec![
        ResponseTemplate::new(401).set_body_string(r#"{"error":{"message":"invalid key"}}"#),
    ])
    .await;
    let (sleep, sleeps) = recorder();
    let error = client(&server, 2, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("x")]))
        .await
        .unwrap_err();

    assert_eq!(error.status_code, Some(401));
    assert!(!error.retryable);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(sleeps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn falls_back_without_response_format_when_endpoint_rejects_it() {
    let server = server_with(vec![
        ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"message":"response_format is not supported by this model"}}"#,
        ),
        ResponseTemplate::new(200).set_body_string(simple_reply()),
        ResponseTemplate::new(200).set_body_string(simple_reply()),
    ])
    .await;
    let (sleep, sleeps) = recorder();
    let client = client(&server, 2, sleep);
    let request = ChatRequest {
        messages: vec![ChatMessage::user("x")],
        tools: vec![],
        json_mode: true,
    };

    assert_eq!(client.complete(request.clone()).await.unwrap().text, "hi");
    let recorded = bodies(&server).await;
    assert!(recorded[0].get("response_format").is_some());
    assert!(recorded[1].get("response_format").is_none());

    // Remembered for later calls on the same client.
    client.complete(request).await.unwrap();
    let recorded = bodies(&server).await;
    assert!(recorded[2].get("response_format").is_none());
    assert!(sleeps.lock().unwrap().is_empty());
}

#[tokio::test]
async fn other_400_errors_are_not_retried() {
    let server = server_with(vec![
        ResponseTemplate::new(400).set_body_string(r#"{"error":{"message":"bad messages"}}"#),
    ])
    .await;
    let (sleep, _) = recorder();
    let request = ChatRequest {
        messages: vec![ChatMessage::user("x")],
        tools: vec![],
        json_mode: true,
    };
    let error = client(&server, 2, sleep)
        .complete(request)
        .await
        .unwrap_err();

    assert_eq!(error.status_code, Some(400));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn api_key_echoed_by_server_is_redacted() {
    let body = format!(r#"{{"error":{{"message":"key {API_KEY} is not allowed"}}}}"#);
    let server = server_with(vec![ResponseTemplate::new(403).set_body_string(body)]).await;
    let (sleep, _) = recorder();
    let error = client(&server, 2, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("x")]))
        .await
        .unwrap_err();

    assert!(!error.message.contains(API_KEY));
    assert!(error.message.contains("***"));
    assert!(!error.to_string().contains(API_KEY));
}

#[tokio::test]
async fn non_json_error_body_is_redacted() {
    let server = server_with(vec![
        ResponseTemplate::new(401).set_body_string(format!("Bearer {API_KEY} rejected")),
    ])
    .await;
    let (sleep, _) = recorder();
    let error = client(&server, 2, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("x")]))
        .await
        .unwrap_err();
    assert!(!error.message.contains(API_KEY));
}

#[tokio::test]
async fn missing_choices_is_an_error() {
    let server = server_with(vec![
        ResponseTemplate::new(200).set_body_string(r#"{"object":"chat.completion","choices":[]}"#),
    ])
    .await;
    let (sleep, _) = recorder();
    let error = client(&server, 2, sleep)
        .complete(ChatRequest::new(vec![ChatMessage::user("x")]))
        .await
        .unwrap_err();
    assert_eq!(error.status_code, Some(200));
}

#[tokio::test]
async fn retries_transport_errors() {
    let mut config = ModelConfig::new(
        ModelRole::Chat,
        "http://127.0.0.1:1/api/v3",
        "test-model",
        API_KEY,
    );
    config.timeout_seconds = 5;
    let (sleep, sleeps) = recorder();
    let client = OpenAiCompatibleClient::with_options(
        config,
        ClientOptions {
            max_retries: 1,
            base_delay_millis: 100,
            sleep,
            http: None,
        },
    );
    let error = client
        .complete(ChatRequest::new(vec![ChatMessage::user("x")]))
        .await
        .unwrap_err();

    assert!(error.retryable);
    assert!(
        error
            .message
            .starts_with("Request to http://127.0.0.1:1/api/v3/chat/completions failed")
    );
    assert!(!error.message.contains(API_KEY));
    assert_eq!(*sleeps.lock().unwrap(), vec![100]);
}

#[tokio::test]
async fn cancelling_the_future_aborts_the_request() {
    let server = server_with(vec![
        ResponseTemplate::new(200)
            .set_body_string(simple_reply())
            .set_delay(std::time::Duration::from_secs(30)),
    ])
    .await;
    let (sleep, _) = recorder();
    let client = client(&server, 0, sleep);
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        client.complete(ChatRequest::new(vec![ChatMessage::user("x")])),
    )
    .await;

    assert!(
        result.is_err(),
        "the in-flight request must be cancelled, not awaited"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn factory_creates_clients_for_a_config() {
    let config = ModelConfig::new(
        ModelRole::Chat,
        "http://127.0.0.1:1/api/v3",
        "test-model",
        API_KEY,
    );
    let client = xiaoman_llm::LlmClients::new().create(config);

    assert_eq!(client.config().role, ModelRole::Chat);
    assert_eq!(client.config().model, "test-model");
    assert_eq!(client.config().base_url, "http://127.0.0.1:1/api/v3");
}

#[tokio::test]
async fn lists_models_in_order_without_duplicates() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/models"))
        .and(wiremock::matchers::header(
            "Authorization",
            format!("Bearer {API_KEY}"),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [
                {"id": "doubao-pro"},
                {"id": "doubao-lite"},
                {"id": "doubao-pro"},
                {"name": "named-model"},
                {"model": "labelled-model"},
                {"object": "model"}
            ]
        })))
        .mount(&server)
        .await;
    let client = client(&server, 0, recorder().0);

    let models = client.list_models().await.expect("models");

    assert_eq!(
        models,
        vec!["doubao-pro", "doubao-lite", "named-model", "labelled-model"]
    );
}

#[tokio::test]
async fn an_empty_model_list_is_ok() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&server)
        .await;
    let client = client(&server, 0, recorder().0);

    assert!(client.list_models().await.expect("models").is_empty());
}

#[tokio::test]
async fn model_list_errors_keep_the_status_and_drop_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/models"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {"message": "invalid key sk-secret-123456"}
        })))
        .mount(&server)
        .await;
    let client = client(&server, 0, recorder().0);

    let error = client.list_models().await.expect_err("401 must fail");

    assert_eq!(error.status_code, Some(401));
    assert!(!error.retryable);
    assert!(!error.message.contains(API_KEY), "{}", error.message);
    assert!(!error.message.contains("sk-secret"), "{}", error.message);
}

#[tokio::test]
async fn a_model_list_that_is_not_json_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/models"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>no</html>"))
        .mount(&server)
        .await;
    let client = client(&server, 0, recorder().0);

    let error = client.list_models().await.expect_err("not JSON must fail");

    assert_eq!(error.status_code, Some(200));
    assert!(!error.retryable);
}
