//! The Settings screen's two probes against a mock OpenAI-compatible endpoint.

use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use xiaoman_app::model_probe;
use xiaoman_app::settings::ModelSettings;

fn settings(server: &MockServer) -> ModelSettings {
    ModelSettings {
        base_url: format!("{}/api/v3", server.uri()),
        model: "doubao-pro".to_string(),
        api_key: "sk-test-key".to_string(),
        ..ModelSettings::default()
    }
}

#[tokio::test]
async fn fetch_models_reads_the_endpoint_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/models"))
        .and(header("authorization", "Bearer sk-test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "doubao-pro"}, {"id": "doubao-lite"}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let models = model_probe::fetch_models(&settings(&server))
        .await
        .expect("models");

    assert_eq!(models, vec!["doubao-pro", "doubao-lite"]);
}

#[tokio::test]
async fn test_model_sends_one_chat_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": "pong"}, "finish_reason": "stop"}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    model_probe::test_model(&settings(&server))
        .await
        .expect("the endpoint answers");

    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
    assert_eq!(body["model"], "doubao-pro");
}

#[test]
fn a_probe_outside_a_tokio_runtime_reports_an_error_instead_of_panicking() {
    use std::future::Future;

    let settings = ModelSettings {
        base_url: "http://127.0.0.1:1/api/v3".to_string(),
        model: "doubao-pro".to_string(),
        api_key: "sk-test-key".to_string(),
        ..ModelSettings::default()
    };
    let mut future = Box::pin(model_probe::fetch_models(&settings));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());

    let poll = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        future.as_mut().poll(&mut context)
    }));

    match poll.expect("polling a probe without a runtime must not panic") {
        std::task::Poll::Ready(Err(message)) => {
            assert!(message.contains("会话线程"), "{message}");
        }
        other => panic!("expected a runtime error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failing_probe_reports_the_error_without_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/models"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {"message": "invalid key sk-test-key"}
        })))
        .mount(&server)
        .await;

    let error = model_probe::fetch_models(&settings(&server))
        .await
        .expect_err("401 fails");

    assert!(!error.contains("sk-test-key"), "{error}");
    assert!(!error.is_empty());
}

#[test]
fn a_model_list_becomes_the_count_and_the_picker() {
    let outcome = model_probe::models_outcome(Ok(vec!["a".to_string(), "b".to_string()]));
    assert_eq!(outcome.message, "获取到 2 个模型");
    assert!(outcome.ok);
    assert_eq!(outcome.models, Some(vec!["a".to_string(), "b".to_string()]));
}

#[test]
fn an_empty_or_failed_fetch_clears_the_list() {
    let empty = model_probe::models_outcome(Ok(Vec::new()));
    assert_eq!(empty.message, "没有获取到模型");
    assert!(!empty.ok);
    assert_eq!(empty.models, Some(Vec::new()));

    let failed = model_probe::models_outcome(Err("HTTP 401".to_string()));
    assert_eq!(failed.message, "HTTP 401");
    assert!(!failed.ok);
    assert_eq!(failed.models, Some(Vec::new()));
}

#[test]
fn a_test_success_says_connected_and_never_touches_the_list() {
    let outcome = model_probe::test_outcome(Ok(()));
    assert_eq!(outcome.message, "连接成功");
    assert!(outcome.ok);
    assert_eq!(outcome.models, None, "测试不该动模型列表");

    let failed = model_probe::test_outcome(Err("连接超时".to_string()));
    assert_eq!(failed.message, "连接超时");
    assert!(!failed.ok);
    assert_eq!(failed.models, None);
}
