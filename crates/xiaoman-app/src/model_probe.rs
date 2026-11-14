//! One-off probes the Settings screen runs against an endpoint as typed.
//!
//! These sit outside [`crate::session`]: the screen probes the draft being edited, which may not
//! be saved yet. Both build a throwaway client from [`ModelSettings`].
//!
//! reqwest needs a tokio reactor, so these must be polled inside a runtime — the screens run them
//! through [`crate::session::Session::on_runtime`], never on plain background threads.

use xiaoman_api::{ChatMessage, ChatRequest, LlmClient, ModelRole};
use xiaoman_llm::OpenAiCompatibleClient;

use crate::settings::ModelSettings;

/// The message a probe reports when it was polled outside a tokio runtime. Without this guard
/// reqwest panics, which takes the process down.
async fn ensure_runtime() -> Result<(), String> {
    if tokio::runtime::Handle::try_current().is_err() {
        return Err("网络请求需要在会话线程上运行".to_string());
    }
    Ok(())
}

/// The model ids `/models` reports for one endpoint, in server order.
pub async fn fetch_models(settings: &ModelSettings) -> Result<Vec<String>, String> {
    ensure_runtime().await?;
    let client = OpenAiCompatibleClient::new(settings.to_model_config(ModelRole::Chat));
    client.list_models().await.map_err(|error| error.message)
}

/// Sends the smallest chat request, to prove the endpoint, key and model name all work.
pub async fn test_model(settings: &ModelSettings) -> Result<(), String> {
    ensure_runtime().await?;
    let client = OpenAiCompatibleClient::new(settings.to_model_config(ModelRole::Chat));
    client
        .complete(ChatRequest::new(vec![ChatMessage::user("ping")]))
        .await
        .map(|_| ())
        .map_err(|error| error.message)
}

/// What a settings screen shows after a probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    /// `None` leaves the model list the screen already shows alone; `Some` replaces it, possibly
    /// with an empty list.
    pub models: Option<Vec<String>>,
    pub message: String,
    /// Success paints the message in the normal colour, failure in the error colour.
    pub ok: bool,
}

/// `获取模型`: a list fills the model picker; an empty list or an error clears it, because the
/// list on screen no longer describes this endpoint.
pub fn models_outcome(result: Result<Vec<String>, String>) -> ProbeOutcome {
    match result {
        Ok(models) if models.is_empty() => ProbeOutcome {
            models: Some(Vec::new()),
            message: "没有获取到模型".to_string(),
            ok: false,
        },
        Ok(models) => ProbeOutcome {
            message: format!("获取到 {} 个模型", models.len()),
            models: Some(models),
            ok: true,
        },
        Err(error) => ProbeOutcome {
            models: Some(Vec::new()),
            message: error,
            ok: false,
        },
    }
}

/// `测试`: one minimal request; the model list stays as it is.
pub fn test_outcome(result: Result<(), String>) -> ProbeOutcome {
    match result {
        Ok(()) => ProbeOutcome {
            models: None,
            message: "连接成功".to_string(),
            ok: true,
        },
        Err(error) => ProbeOutcome {
            models: None,
            message: error,
            ok: false,
        },
    }
}
