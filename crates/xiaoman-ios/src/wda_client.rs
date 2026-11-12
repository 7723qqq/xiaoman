//! HTTP transport to WebDriverAgent, with a lazy session and one retry after an invalid session.

use std::error::Error;
use std::fmt;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

/// WDA failure without request bodies or potentially sensitive response messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WdaError {
    pub endpoint: String,
    pub status_code: u16,
    pub error: Option<String>,
}

impl WdaError {
    pub fn new(
        endpoint: impl Into<String>,
        status_code: u16,
        error: Option<impl Into<String>>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            status_code,
            error: error.map(Into::into),
        }
    }

    fn transport(endpoint: &str) -> Self {
        Self::new(endpoint, 0, Some("transport failure"))
    }
}

impl fmt::Display for WdaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WDA {}: HTTP {}", self.endpoint, self.status_code)?;
        if let Some(error) = &self.error {
            write!(f, " ({error})")?;
        }
        Ok(())
    }
}

impl Error for WdaError {}

/// HTTP transport with a lazy session and one retry after an invalid session.
///
/// Every session-scoped call creates the WDA session on first use. A `404 invalid session id`
/// rebuilds the session once and repeats the call; any other failure is returned to the caller.
pub struct WdaClient {
    base: String,
    timeout: Duration,
    http: reqwest::Client,
    session: Mutex<Option<String>>,
}

impl WdaClient {
    /// Client for `base_url` with the default 30 s timeout.
    pub fn new(base_url: impl Into<String>) -> Result<Self, WdaError> {
        Self::with_timeout(base_url, Duration::from_secs(30))
    }

    /// Client for `base_url`; `timeout` must be positive.
    pub fn with_timeout(base_url: impl Into<String>, timeout: Duration) -> Result<Self, WdaError> {
        if timeout.is_zero() {
            return Err(WdaError::new("", 0, Some("timeout must be positive")));
        }
        let http = reqwest::Client::builder()
            .timeout(timeout)
            // A local device controller must never go through the system proxy.
            .no_proxy()
            .build()
            .map_err(|_| WdaError::new("", 0, Some("cannot build the HTTP client")))?;
        Ok(Self {
            base: base_url.into().trim_end_matches('/').to_string(),
            timeout,
            http,
            session: Mutex::new(None),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub async fn status(&self) -> Result<Value, WdaError> {
        self.request("GET", |_| "/status".to_string(), None, false)
            .await
    }

    pub async fn window_size(&self) -> Result<Map<String, Value>, WdaError> {
        let value = self
            .request(
                "GET",
                |session| format!("/session/{}/window/size", session),
                None,
                true,
            )
            .await?;
        match value {
            Value::Object(map) => Ok(map),
            _ => Err(WdaError::new(
                "/window/size",
                200,
                Some("invalid window size"),
            )),
        }
    }

    pub async fn screenshot(&self) -> Result<String, WdaError> {
        string_value(
            self.request("GET", |_| "/screenshot".to_string(), None, false)
                .await?,
            "/screenshot",
        )
    }

    pub async fn source(&self) -> Result<String, WdaError> {
        string_value(
            self.request("GET", |_| "/source?format=xml".to_string(), None, false)
                .await?,
            "/source?format=xml",
        )
    }

    pub async fn active_app_info(&self) -> Result<Map<String, Value>, WdaError> {
        match self
            .request("GET", |_| "/wda/activeAppInfo".to_string(), None, false)
            .await?
        {
            Value::Object(map) => Ok(map),
            _ => Err(WdaError::new(
                "/wda/activeAppInfo",
                200,
                Some("invalid app info"),
            )),
        }
    }

    pub async fn actions(&self, body: Value) -> Result<(), WdaError> {
        self.request(
            "POST",
            |session| format!("/session/{}/actions", session),
            Some(&body),
            true,
        )
        .await
        .map(|_| ())
    }

    pub async fn keys(&self, values: &[String]) -> Result<(), WdaError> {
        let body = json!({ "value": values });
        self.request(
            "POST",
            |session| format!("/session/{}/wda/keys", session),
            Some(&body),
            true,
        )
        .await
        .map(|_| ())
    }

    /// Id of the element with keyboard focus, or `None` when WDA knows no focused element.
    pub async fn focused_element(&self) -> Result<Option<String>, WdaError> {
        let body = json!({ "using": "predicate string", "value": "hasKeyboardFocus == 1" });
        let value = self
            .request(
                "POST",
                |session| format!("/session/{}/elements", session),
                Some(&body),
                true,
            )
            .await?;
        let Some(element) = value
            .as_array()
            .and_then(|elements| elements.first())
            .and_then(Value::as_object)
        else {
            return Ok(None);
        };
        for key in ["ELEMENT", "element-6066-11e4-a52e-4f735466cecf"] {
            if let Some(id) = element.get(key).and_then(json_content) {
                if !id.trim().is_empty() {
                    return Ok(Some(id));
                }
            }
        }
        Ok(None)
    }

    pub async fn clear(&self, element_id: &str) -> Result<(), WdaError> {
        self.request(
            "POST",
            |session| format!("/session/{}/element/{element_id}/clear", session),
            Some(&json!({})),
            true,
        )
        .await
        .map(|_| ())
    }

    /// Sets the element's value one Unicode scalar at a time, so supplementary characters survive.
    pub async fn set_value(&self, element_id: &str, text: &str) -> Result<(), WdaError> {
        let characters: Vec<String> = text
            .chars()
            .map(|character| character.to_string())
            .collect();
        let body = json!({ "value": characters });
        self.request(
            "POST",
            |session| format!("/session/{}/element/{element_id}/value", session),
            Some(&body),
            true,
        )
        .await
        .map(|_| ())
    }

    pub async fn home(&self) -> Result<(), WdaError> {
        self.request(
            "POST",
            |_| "/wda/homescreen".to_string(),
            Some(&json!({})),
            false,
        )
        .await
        .map(|_| ())
    }

    pub async fn launch_app(&self, bundle_id: &str) -> Result<(), WdaError> {
        let body = json!({ "bundleId": bundle_id });
        self.request(
            "POST",
            |session| format!("/session/{}/wda/apps/launch", session),
            Some(&body),
            true,
        )
        .await
        .map(|_| ())
    }

    pub async fn open_deeplink(&self, url: &str) -> Result<(), WdaError> {
        let body = json!({ "url": url });
        self.request(
            "POST",
            |session| format!("/session/{}/url", session),
            Some(&body),
            true,
        )
        .await
        .map(|_| ())
    }

    /// Sends one call, retrying once after rebuilding the session.
    ///
    /// `path` receives the session id for scoped calls and `None` for calls that do not need one.
    async fn request<F>(
        &self,
        method: &str,
        path: F,
        body: Option<&Value>,
        scoped: bool,
    ) -> Result<Value, WdaError>
    where
        F: Fn(&str) -> String,
    {
        let mut retried = false;
        loop {
            let session = if scoped {
                Some(self.session().await?)
            } else {
                None
            };
            let endpoint = path(session.as_deref().unwrap_or_default());
            match self.send(method, &endpoint, body).await {
                Ok(response) => return Ok(response.get("value").cloned().unwrap_or(Value::Null)),
                Err(error) => {
                    let retry = !retried
                        && error.status_code == 404
                        && error.error.as_deref() == Some("invalid session id");
                    if !retry {
                        return Err(error);
                    }
                    *self.session.lock().await = None;
                    self.session().await?;
                    retried = true;
                }
            }
        }
    }

    async fn session(&self) -> Result<String, WdaError> {
        let mut session = self.session.lock().await;
        if let Some(id) = session.as_ref() {
            return Ok(id.clone());
        }
        let body = json!({ "capabilities": { "alwaysMatch": {} } });
        let response = self.send("POST", "/session", Some(&body)).await?;
        let id = json_content(&response["sessionId"])
            .or_else(|| {
                response
                    .get("value")
                    .and_then(|value| value.get("sessionId"))
                    .and_then(json_content)
            })
            .ok_or_else(|| WdaError::new("/session", 200, Some("missing session id")))?;
        if id.trim().is_empty() {
            return Err(WdaError::new("/session", 200, Some("empty session id")));
        }
        *session = Some(id.clone());
        Ok(id)
    }

    async fn send(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<&Value>,
    ) -> Result<Value, WdaError> {
        let url = format!("{}{}", self.base, endpoint);
        let request = self
            .http
            .request(method.parse().unwrap_or(reqwest::Method::GET), url)
            .header(reqwest::header::ACCEPT, "application/json");
        let request = if method == "POST" {
            request.header(reqwest::header::CONTENT_TYPE, "application/json")
        } else {
            request
        };
        let request = match body {
            Some(body) => request.body(serde_json::to_vec(body).unwrap_or_default()),
            None => request,
        };
        let response = request
            .send()
            .await
            .map_err(|_| WdaError::transport(endpoint))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|_| WdaError::transport(endpoint))?;
        let json: Value = serde_json::from_str(&text)
            .map_err(|_| WdaError::new(endpoint, status, Some("invalid JSON")))?;
        if !json.is_object() {
            return Err(WdaError::new(endpoint, status, Some("invalid JSON")));
        }
        let error = json
            .get("value")
            .and_then(Value::as_object)
            .and_then(|value| value.get("error"))
            .and_then(json_content);
        if !(200..=299).contains(&status) || error.is_some() {
            return Err(WdaError::new(endpoint, status, error));
        }
        Ok(json)
    }
}

fn string_value(value: Value, endpoint: &str) -> Result<String, WdaError> {
    match value {
        Value::String(text) => Ok(text),
        _ => Err(WdaError::new(endpoint, 200, Some("invalid payload"))),
    }
}

/// `contentOrNull` of kotlinx.serialization: strings as-is, other scalars rendered, null/container
/// as `None`. JSON `null` is a `JsonPrimitive` there but `contentOrNull` still yields `None`.
pub(crate) fn json_content(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}
