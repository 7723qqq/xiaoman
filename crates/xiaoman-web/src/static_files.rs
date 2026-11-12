//! The page itself: `tower-http`'s [`ServeDir`] with the headers and error bodies the Kotlin
//! server returned.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use percent_encoding::percent_decode_str;
use tower::ServiceExt;
use tower_http::services::ServeDir;

use crate::problem::Problem;

pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; connect-src 'self'";

pub struct StaticFiles {
    directory: ServeDir,
}

impl StaticFiles {
    pub fn new(root: PathBuf) -> Self {
        Self {
            directory: ServeDir::new(root).append_index_html_on_directories(false),
        }
    }

    /// Serves one file: `/` is the page, `/claim` the self-service form, and only fixed names exist.
    pub async fn serve(&self, request: Request<Body>) -> Response {
        if request.method() != axum::http::Method::GET {
            return Problem::new(StatusCode::METHOD_NOT_ALLOWED, "请求方法不支持").into_response();
        }
        let raw = request.uri().path().to_string();
        let decoded = percent_decode_str(&raw).decode_utf8_lossy().to_string();
        if decoded.contains("..") || decoded.contains('\\') || decoded.contains('\0') {
            return Problem::bad_request("不允许访问此路径").into_response();
        }
        // `/` is the page and `/claim` the self-service form; every other path is the file itself.
        let (forwarded, name) = match decoded.as_str() {
            "/" | "" => ("/index.html".to_string(), "/index.html".to_string()),
            "/claim" | "/claim/" => ("/claim.html".to_string(), "/claim.html".to_string()),
            other => (raw.clone(), other.to_string()),
        };
        let Ok(uri) = forwarded.parse::<axum::http::Uri>() else {
            return Problem::bad_request("不允许访问此路径").into_response();
        };
        let forwarded = Request::builder()
            .method(axum::http::Method::GET)
            .uri(uri)
            .body(Body::empty())
            .expect("a GET request with a path is always valid");
        match self.directory.clone().oneshot(forwarded).await {
            Ok(response) => {
                if response.status() == StatusCode::NOT_FOUND {
                    return Problem::not_found("文件不存在").into_response();
                }
                let mut response = response.map(Body::new);
                if let Ok(value) = header::HeaderValue::from_str(content_type(&name)) {
                    response.headers_mut().insert(header::CONTENT_TYPE, value);
                }
                if name.to_lowercase().ends_with(".html") {
                    response.headers_mut().insert(
                        header::CONTENT_SECURITY_POLICY,
                        header::HeaderValue::from_static(CONTENT_SECURITY_POLICY),
                    );
                }
                response
            }
            Err(_) => Problem::internal().into_response(),
        }
    }
}

pub fn content_type(name: &str) -> &'static str {
    let extension = name.rsplit('.').next().unwrap_or_default().to_lowercase();
    match extension.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "webmanifest" => "application/manifest+json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_match_the_kotlin_table() {
        assert_eq!(content_type("/index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("app.js"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("style.CSS"), "text/css; charset=utf-8");
        assert_eq!(
            content_type("manifest.webmanifest"),
            "application/manifest+json; charset=utf-8"
        );
        assert_eq!(content_type("icon.svg"), "image/svg+xml");
        assert_eq!(content_type("apple-touch-icon.png"), "image/png");
        assert_eq!(content_type("no-extension"), "application/octet-stream");
    }
}
