//! The JSON error every API route returns, and the response helpers built on it.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// One failed request; `message` is shown to the page as `{"error": message}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub status: StatusCode,
    pub message: String,
}

impl Problem {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "需要访问码")
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    pub fn too_large() -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, "请求内容超过 256KB")
    }

    pub fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "请求处理失败")
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        json_response(self.status, &json!({ "error": self.message }))
    }
}

/// A JSON response with the charset every client of the page expects.
pub fn json_response(status: StatusCode, value: &Value) -> Response {
    let body = value.to_string();
    (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        body,
    )
        .into_response()
}
