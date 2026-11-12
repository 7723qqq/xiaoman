//! Web glue over `xiaoman-mcp`'s tool provider.
//!
//! Discovery, approvals, name sanitizing, redaction, truncation and the retry-after delay all live
//! in `xiaoman-mcp`; the web only injects its own transport factory (tests replace servers there)
//! and its clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use xiaoman_api::ToolApprovalGateway;
use xiaoman_mcp::{McpConfig, McpError, McpServerConfig, McpTransport};

use crate::memory::Clock;

pub use xiaoman_mcp::{McpToolProvider, ServerStatus};

/// Builds the transport for one server; tests substitute fake servers here.
pub type TransportFactory =
    Arc<dyn Fn(&McpServerConfig) -> Result<Arc<dyn McpTransport>, McpError> + Send + Sync>;

/// Caps how many stdio MCP child processes run at once, across every web session.
///
/// The Kotlin `StdioMcp` counts the same way: HTTP servers are not counted, a stdio server over
/// the cap fails with the "人数已满" message, and closing (or dropping) a transport releases the
/// slot exactly once.
pub struct StdioLimiter {
    max: usize,
    running: AtomicUsize,
    start: TransportFactory,
}

impl StdioLimiter {
    pub fn new(max: usize) -> Arc<Self> {
        Self::with_start(max, Arc::new(xiaoman_mcp::create))
    }

    pub fn with_start(max: usize, start: TransportFactory) -> Arc<Self> {
        Arc::new(Self {
            max,
            running: AtomicUsize::new(0),
            start,
        })
    }

    pub fn running(&self) -> usize {
        self.running.load(Ordering::SeqCst)
    }

    /// Starts one stdio server within the cap, or opens an HTTP transport without counting it.
    pub fn transport(
        self: &Arc<Self>,
        server: &McpServerConfig,
    ) -> Result<Arc<dyn McpTransport>, McpError> {
        if server.url.is_some() {
            return (self.start)(server);
        }
        if self.running.fetch_add(1, Ordering::SeqCst) >= self.max {
            self.running.fetch_sub(1, Ordering::SeqCst);
            return Err(McpError(format!(
                "{} 当前使用人数已满，请稍后再试",
                server.title
            )));
        }
        match (self.start)(server) {
            Ok(inner) => Ok(Arc::new(LimitedTransport {
                inner,
                limiter: Arc::clone(self),
                released: AtomicBool::new(false),
            })),
            Err(error) => {
                self.running.fetch_sub(1, Ordering::SeqCst);
                Err(error)
            }
        }
    }

    fn release(&self) {
        self.running.fetch_sub(1, Ordering::SeqCst);
    }
}

struct LimitedTransport {
    inner: Arc<dyn McpTransport>,
    limiter: Arc<StdioLimiter>,
    released: AtomicBool,
}

impl LimitedTransport {
    fn release(&self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            self.limiter.release();
        }
    }
}

#[async_trait::async_trait]
impl McpTransport for LimitedTransport {
    async fn exchange(
        &self,
        message: serde_json::Value,
    ) -> Result<Option<serde_json::Value>, McpError> {
        self.inner.exchange(message).await
    }

    fn close(&self) {
        self.release();
        self.inner.close();
    }
}

impl Drop for LimitedTransport {
    fn drop(&mut self) {
        self.release();
    }
}

/// A provider that reaches servers through `transports` and stamps failures with `clock`.
pub fn build_provider(
    config: McpConfig,
    approvals: Arc<dyn ToolApprovalGateway>,
    transports: &TransportFactory,
    clock: &Clock,
) -> McpToolProvider {
    let factory = transports.clone();
    let clock = clock.clone();
    McpToolProvider::new(config, approvals)
        .with_transport_factory(move |server| factory(server))
        .with_clock(move || clock())
}

#[cfg(test)]
mod tests {
    use xiaoman_api::ToolProvider;

    use super::*;

    #[tokio::test]
    async fn the_injected_factory_and_clock_reach_the_provider() {
        let mut server = McpServerConfig::new("broken");
        server.url = Some("https://127.0.0.1:1/mcp".to_string());
        server.timeout_seconds = 1;
        let config = McpConfig {
            servers: vec![server],
            warnings: Vec::new(),
        };
        let transports: TransportFactory =
            Arc::new(|_server| Err(McpError("连接失败".to_string())));
        let clock: Clock = Arc::new(|| 0);
        let provider = build_provider(
            config,
            Arc::new(crate::channels::WebChannels::new()),
            &transports,
            &clock,
        );
        assert!(provider.specs().await.is_empty());
        let status = provider.status(false).await;
        assert_eq!(status.len(), 1);
        assert!(!status[0].connected);
        assert_eq!(status[0].error.as_deref(), Some("连接失败"));
        assert!(provider.call("task", "broken__read", "{}").await.is_none());
        provider.close();
    }

    struct FakeStdio {
        closed: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl McpTransport for FakeStdio {
        async fn exchange(
            &self,
            _message: serde_json::Value,
        ) -> Result<Option<serde_json::Value>, McpError> {
            Ok(None)
        }

        fn close(&self) {
            self.closed.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn fake_factory(closed: Arc<AtomicUsize>) -> TransportFactory {
        Arc::new(move |_server: &McpServerConfig| {
            Ok(Arc::new(FakeStdio {
                closed: closed.clone(),
            }) as Arc<dyn McpTransport>)
        })
    }

    #[test]
    fn stdio_processes_are_capped_globally_and_release_once() {
        let closed = Arc::new(AtomicUsize::new(0));
        let limiter = StdioLimiter::with_start(2, fake_factory(closed.clone()));
        let mut server = McpServerConfig::new("12306");
        server.command = Some("npx".to_string());

        let a = limiter.transport(&server).expect("first");
        let b = limiter.transport(&server).expect("second");
        let error = match limiter.transport(&server) {
            Ok(_) => panic!("the third is over the cap"),
            Err(error) => error,
        };
        assert!(error.0.contains("人数已满"), "{}", error.0);
        assert_eq!(2, limiter.running());

        a.close();
        a.close(); // close is idempotent; the fake counts both calls, the slot is released once
        assert_eq!(1, limiter.running());
        assert_eq!(2, closed.load(Ordering::SeqCst));

        let c = limiter.transport(&server).expect("a freed slot is reused");
        assert_eq!(2, limiter.running());
        drop(c);
        assert_eq!(
            1,
            limiter.running(),
            "dropping a transport releases its slot"
        );
        drop(b);
        assert_eq!(0, limiter.running());
    }

    #[test]
    fn http_servers_are_not_counted_and_a_failed_start_does_not_leak() {
        let closed = Arc::new(AtomicUsize::new(0));
        let limiter = StdioLimiter::with_start(1, fake_factory(closed));
        let mut http = McpServerConfig::new("web");
        http.url = Some("https://example.com/mcp".to_string());
        let transport = limiter.transport(&http).expect("http");
        assert_eq!(0, limiter.running());
        drop(transport);

        let failing: TransportFactory =
            Arc::new(|_server: &McpServerConfig| Err(McpError("启动失败".to_string())));
        let limiter = StdioLimiter::with_start(1, failing);
        let mut stdio = McpServerConfig::new("12306");
        stdio.command = Some("npx".to_string());
        assert!(limiter.transport(&stdio).is_err());
        assert_eq!(0, limiter.running(), "a failed start does not hold a slot");
    }
}
