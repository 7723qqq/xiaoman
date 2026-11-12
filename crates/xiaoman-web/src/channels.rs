//! Questions and tool approvals of one chat stream: the agent waits, the page answers through
//! `/api/answer`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use xiaoman_api::{ToolApprovalGateway, ToolApprovalRequest, UserChannel};

use crate::random;

pub const DEFAULT_TIMEOUT_MILLIS: u64 = 180_000;

/// Text returned when the page never answers a question.
pub const NO_ANSWER: &str = "用户没有回答";

struct Pending {
    approval: bool,
    answer: oneshot::Sender<Value>,
}

/// Pending answers belong to one session and one chat stream.
pub struct WebChannels {
    timeout_millis: u64,
    pending: Mutex<HashMap<String, Pending>>,
    output: Mutex<Option<mpsc::UnboundedSender<Value>>>,
}

impl WebChannels {
    pub fn new() -> Self {
        Self::with_timeout(DEFAULT_TIMEOUT_MILLIS)
    }

    pub fn with_timeout(timeout_millis: u64) -> Self {
        Self {
            timeout_millis,
            pending: Mutex::new(HashMap::new()),
            output: Mutex::new(None),
        }
    }

    pub fn attach(&self, output: mpsc::UnboundedSender<Value>) {
        *self.output.lock().unwrap() = Some(output);
    }

    pub fn emit(&self, value: Value) {
        if let Ok(output) = self.output.lock() {
            if let Some(output) = output.as_ref() {
                let _ = output.send(value);
            }
        }
    }

    async fn wait_for(&self, approval: bool, make: impl FnOnce(&str) -> Value) -> Option<Value> {
        let id = random::uuid();
        let (sender, receiver) = oneshot::channel();
        {
            let output = self.output.lock().unwrap();
            let output = output.as_ref()?;
            self.pending.lock().unwrap().insert(
                id.clone(),
                Pending {
                    approval,
                    answer: sender,
                },
            );
            let _ = output.send(make(&id));
        }
        let received =
            tokio::time::timeout(Duration::from_millis(self.timeout_millis), receiver).await;
        self.pending.lock().unwrap().remove(&id);
        match received {
            Ok(Ok(value)) => Some(value),
            _ => None,
        }
    }

    /// Answers a pending question or approval. False when the id is unknown or the body cannot
    /// answer that kind of request.
    pub fn answer(&self, body: &Value) -> bool {
        let Some(id) = body.get("id").and_then(Value::as_str) else {
            return false;
        };
        let Some(pending) = self.pending.lock().unwrap().remove(id) else {
            return false;
        };
        let valid = if pending.approval {
            body.get("approved").and_then(Value::as_bool).is_some()
        } else {
            body.get("text").is_some_and(Value::is_string)
        };
        if !valid {
            self.pending.lock().unwrap().insert(id.to_string(), pending);
            return false;
        }
        pending.answer.send(body.clone()).is_ok()
    }

    /// Drops the output and rejects every pending request; a turn that ends without the page.
    pub fn detach(&self) {
        *self.output.lock().unwrap() = None;
        let pending: Vec<Pending> = self
            .pending
            .lock()
            .unwrap()
            .drain()
            .map(|(_, pending)| pending)
            .collect();
        for pending in pending {
            let _ = pending.answer.send(json!({ "approved": false }));
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

impl Default for WebChannels {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl UserChannel for WebChannels {
    async fn ask(&self, _task_id: &str, question: &str, _takeover: bool) -> String {
        self.wait_for(
            false,
            |id| json!({ "type": "question", "id": id, "question": question }),
        )
        .await
        .and_then(|value| {
            value
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| NO_ANSWER.to_string())
    }
}

#[async_trait]
impl ToolApprovalGateway for WebChannels {
    async fn approve(&self, request: ToolApprovalRequest) -> bool {
        self.wait_for(true, |id| {
            json!({
                "type": "approval",
                "id": id,
                "title": request.title.clone().unwrap_or_else(|| request.tool.clone()),
                "server": request.server,
                "tool": request.tool,
                "arguments": request.arguments_json,
                "reason": request.reason,
            })
        })
        .await
        .and_then(|value| value.get("approved").and_then(Value::as_bool))
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn request() -> ToolApprovalRequest {
        ToolApprovalRequest {
            task_id: "task".into(),
            server: "server".into(),
            tool: "write".into(),
            title: None,
            arguments_json: "{}".into(),
            reason: "修改".into(),
        }
    }

    #[tokio::test]
    async fn answers_are_session_scoped_and_wrong_shapes_are_rejected() {
        let channels = Arc::new(WebChannels::with_timeout(40));
        let (sender, mut receiver) = mpsc::unbounded_channel();
        channels.attach(sender);
        let waiting = tokio::spawn({
            let channels = channels.clone();
            async move { channels.approve(request()).await }
        });
        let approval = receiver.recv().await.unwrap();
        assert_eq!(approval["type"], "approval");
        assert_eq!(approval["title"], "write");
        // An approval needs `approved`; a question needs `text`.
        assert!(!channels.answer(&json!({ "id": approval["id"], "text": "回家" })));
        assert!(channels.answer(&json!({ "id": approval["id"], "approved": true })));
        assert!(waiting.await.unwrap());
        assert_eq!(channels.pending_count(), 0);
    }

    #[tokio::test]
    async fn timeout_and_detach_answer_with_defaults() {
        let channels = Arc::new(WebChannels::with_timeout(30));
        let (sender, mut receiver) = mpsc::unbounded_channel();
        channels.attach(sender);
        let asking = tokio::spawn({
            let channels = channels.clone();
            async move { channels.ask("task", "去哪里？", false).await }
        });
        assert_eq!(receiver.recv().await.unwrap()["type"], "question");
        assert_eq!(asking.await.unwrap(), NO_ANSWER);
        assert_eq!(channels.pending_count(), 0);

        let asking = tokio::spawn({
            let channels = channels.clone();
            async move { channels.ask("task", "还有吗？", false).await }
        });
        assert_eq!(receiver.recv().await.unwrap()["type"], "question");
        channels.detach();
        assert_eq!(asking.await.unwrap(), NO_ANSWER);
        assert_eq!(channels.pending_count(), 0);
    }
}
