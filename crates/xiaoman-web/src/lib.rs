//! The web server: the page, the `/api/*` endpoints, and the SSE chat stream.
//!
//! Ported from Kotlin `web-server/`. Credentials and conversation state stay in memory; long-term
//! memory lives in the browser and travels with every chat request, so nothing is written to disk.
//!
//! Routes: `GET /api/info`, `GET /api/catalog`, `POST /api/session`, `POST /api/chat` (SSE),
//! `POST /api/answer`, `POST /api/mcp/test`, `POST /api/claim/{start,verify}`, the OpenAI-compatible
//! `GET /v1/models` and `POST /v1/chat/completions` of the class-token mode, and the fixed file
//! server for everything else (including the `/claim` page).
//!
//! Class tokens (`ClassTokens.kt`) and self-service claiming (`ForumClaims.kt`) are ported in
//! [`class_tokens`] and [`claims`]; the admin command lives in [`tokens_admin`]. [`class_gateway`]
//! holds the settings and the metered client both entry points share, and [`class_v1`] the limiter,
//! body limit and OpenAI error envelope of the `/v1` passthrough.

pub mod browser_memory;
pub mod channels;
pub mod claims;
pub mod class_gateway;
pub mod class_tokens;
pub mod class_v1;
pub mod ics;
pub mod local_launch;
pub mod mcp_provider;
pub mod mcp_selection;
pub mod memory;
pub mod options;
pub mod problem;
pub mod random;
pub mod safe_dns;
pub mod server;
pub mod session;
pub mod static_files;
pub mod tokens_admin;

pub use browser_memory::{BrowserMemory, LoadedNotes};
pub use channels::WebChannels;
pub use claims::{ClaimConfig, ClaimService};
pub use class_gateway::ClassGateway;
pub use class_tokens::{ClassDenied, ClassToken, ClassTokenCodec, ClassTokenStore, ForumClaim};
pub use ics::IcsReminders;
pub use mcp_provider::{McpToolProvider, ServerStatus, TransportFactory, build_provider};
pub use mcp_selection::{SelectionOptions, build_mcp_config};
pub use memory::InMemoryMemory;
pub use options::WebOptions;
pub use problem::Problem;
pub use safe_dns::{SafeDns, is_public_ip, is_public_url};
pub use server::{AppState, BoundServer, MAX_BODY_BYTES, ServerError, WebServer};
pub use session::{ChatSkillRepository, LlmFactory, SessionStore, WebSession, llm_factory};
pub use xiaoman_mcp::{
    CatalogKey, CatalogServer, CustomMcpServer, McpCatalog, McpPlatform, McpSelection,
};
