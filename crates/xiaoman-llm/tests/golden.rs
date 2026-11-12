//! The LLM fixtures are shared with the Kotlin `GoldenTest`: the same queued responses produce the
//! same text, error flags, request count, sleeps and JSON-mode fallback.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};
use xiaoman_api::{ChatMessage, ChatRequest, LlmClient, ModelConfig, ModelRole};
use xiaoman_llm::{ClientOptions, OpenAiCompatibleClient};

struct Sequence {
    responses: Mutex<VecDeque<ResponseTemplate>>,
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

#[tokio::test]
async fn retry_and_fallback_match_the_shared_script() {
    let path_to_fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/llm/retry.json");
    let cases: Value =
        serde_json::from_str(&std::fs::read_to_string(path_to_fixtures).expect("fixture"))
            .expect("JSON");
    let cases = cases.as_array().expect("array");
    assert!(cases.len() >= 4, "expected the shared LLM fixtures");
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let server = MockServer::start().await;
        let mut responses = VecDeque::new();
        for response in case["responses"].as_array().expect("responses") {
            let status = response["status"].as_u64().expect("status") as u16;
            let mut template = ResponseTemplate::new(status)
                .set_body_string(response["body"].as_str().expect("body"));
            if let Some(retry_after) = response["retryAfter"].as_str() {
                template = template.insert_header("Retry-After", retry_after);
            }
            responses.push_back(template);
        }
        Mock::given(method("POST"))
            .and(path("/api/v3/chat/completions"))
            .respond_with(Sequence {
                responses: Mutex::new(responses),
            })
            .mount(&server)
            .await;

        let sleeps: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = sleeps.clone();
        let sleep: xiaoman_llm::SleepFn = Arc::new(move |millis| {
            sink.lock().unwrap().push(millis);
            Box::pin(async {})
        });
        let config = ModelConfig::new(
            ModelRole::Chat,
            format!("{}/api/v3", server.uri()),
            "m",
            "sk-test-123456",
        );
        let client = OpenAiCompatibleClient::with_options(
            config,
            ClientOptions {
                max_retries: case["maxRetries"].as_u64().unwrap_or(2) as u32,
                sleep,
                ..Default::default()
            },
        );
        let request = ChatRequest {
            messages: vec![ChatMessage::user("hi")],
            tools: Vec::new(),
            json_mode: case["jsonMode"].as_bool().unwrap_or(false),
        };

        let expected = &case["expected"];
        if expected["ok"].as_bool().expect("ok") {
            let response = client.complete(request).await.expect("ok response");
            assert_eq!(
                expected["text"].as_str().expect("text"),
                response.text,
                "{name}"
            );
        } else {
            let error = client.complete(request).await.expect_err("an error");
            assert_eq!(
                expected["retryable"].as_bool().expect("retryable"),
                error.retryable,
                "{name}"
            );
        }
        let requests = server.received_requests().await.expect("requests");
        assert_eq!(
            expected["attempts"].as_u64().expect("attempts") as usize,
            requests.len(),
            "{name} attempts"
        );
        if let Some(expected_sleeps) = expected["sleeps"].as_array() {
            let expected_sleeps: Vec<u64> = expected_sleeps
                .iter()
                .map(|value| value.as_u64().expect("sleep"))
                .collect();
            assert_eq!(expected_sleeps, *sleeps.lock().unwrap(), "{name} sleeps");
        }
        if expected["lastRequestWithoutResponseFormat"].as_bool() == Some(true) {
            assert!(requests.len() >= 2, "{name}: both requests");
            let first = String::from_utf8_lossy(&requests[0].body);
            let last = String::from_utf8_lossy(&requests[requests.len() - 1].body);
            assert!(
                first.contains("response_format"),
                "{name}: first request asks for JSON"
            );
            assert!(
                !last.contains("response_format"),
                "{name}: fallback request dropped it"
            );
        }
    }
}
