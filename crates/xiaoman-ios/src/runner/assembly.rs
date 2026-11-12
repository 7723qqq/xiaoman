//! Wires the core modules to a WDA-backed iPhone, as `AppGraph` does for the Android app.

use std::io::Write;
use std::sync::Arc;

use xiaoman_agent::{AgentDeps, XiaomanAgent};
use xiaoman_api::{
    AgentConfig, ApprovalGateway, AuditSink, Device, LlmClient, ModelConfig, ModelRole,
    ReminderScheduler, Sentinel, SkillRepository, UserChannel,
};
use xiaoman_llm::LlmClients;
use xiaoman_mcp::{McpConfig, McpToolProvider, UserChannelApprovals};
use xiaoman_memory::FileMemoryStore;
use xiaoman_sentinel::{PolicySentinel, SentinelOptions};
use xiaoman_skills::DirectorySkillRepository;

use super::channels::{ConsoleChannels, Output};
use super::config::RunnerConfig;
use super::reminders::MacReminders;
use super::trace::TraceRecorder;
use crate::ios_platform::IosPlatform;
use crate::wda_client::WdaClient;
use crate::wda_device::{WdaDevice, WdaDeviceOptions};

/// Builds one model client per [`ModelConfig`]; replaceable in tests.
pub type LlmFactory = Arc<dyn Fn(ModelConfig) -> Arc<dyn LlmClient> + Send + Sync>;

/// The default factory: OpenAI-compatible clients sharing one connection pool.
pub fn default_llm_factory() -> LlmFactory {
    let clients = Arc::new(LlmClients::new());
    Arc::new(move |config| Arc::new(clients.create(config)) as Arc<dyn LlmClient>)
}

/// The assembled agent with the pieces the command line needs to reach.
pub struct RunnerSession {
    pub agent: XiaomanAgent,
    pub sentinel: Arc<PolicySentinel>,
    pub trace: Arc<TraceRecorder>,
    pub skill_count: usize,
}

/// Builds the session for one run: memory, skills, sentinel, trace and the WDA device.
pub async fn assemble(
    config: &RunnerConfig,
    channels: Arc<ConsoleChannels>,
    llm: &LlmFactory,
    env: &std::collections::BTreeMap<String, String>,
    out: Output,
) -> Result<RunnerSession, String> {
    let _ = std::fs::create_dir_all(config.memory_dir());
    let _ = std::fs::create_dir_all(config.user_skills_dir());
    let trace = Arc::new(TraceRecorder::new(config.traces_dir(), config.secrets()));

    let skills = Arc::new(DirectorySkillRepository::new(config.skill_dirs()));
    let loaded = skills.reload().await;
    for problem in &loaded.problems {
        let line = problem
            .line
            .map(|line| format!(":{line}"))
            .unwrap_or_default();
        write_line(
            &out,
            &format!("技能问题：{}{line} {}", problem.source, problem.message),
        );
    }

    let approvals: Arc<dyn ApprovalGateway> = channels.clone();
    let audit: Arc<dyn AuditSink> = trace.clone();
    let sentinel = Arc::new(PolicySentinel::with_options(
        SentinelOptions {
            launcher_packages: IosPlatform::launcher_bundles(),
            self_package: IosPlatform::runner_bundle_id(config.wda_bundle_id.as_deref()),
            ..Default::default()
        },
        approvals,
        audit,
    ));

    let client =
        Arc::new(WdaClient::new(config.wda_url.clone()).map_err(|error| error.to_string())?);
    let device = WdaDevice::with_options(client, WdaDeviceOptions::default())
        .map_err(|error| error.to_string())?;
    let device: Arc<dyn Device> = Arc::new(device);
    let device = trace.device(device);

    let users: Arc<dyn UserChannel> = channels;
    let reminders: Arc<dyn ReminderScheduler> = Arc::new(MacReminders::new(out.clone()));

    // The same `mcp/servers.example.json` shape the `tools` and `call` commands read; its tools
    // become callable by the chat model, with the console's approval card gating writes.
    let mcp = McpConfig::load(&config.mcp_config_file(), env);
    for warning in &mcp.warnings {
        write_line(&out, &format!("MCP：{warning}"));
    }
    let provider = if mcp.servers.iter().any(|server| server.enabled) {
        let approvals = Arc::new(UserChannelApprovals::new(users.clone()));
        Some(Arc::new(McpToolProvider::new(mcp, approvals)))
    } else {
        None
    };

    let sentinel_trait: Arc<dyn Sentinel> = sentinel.clone();
    let agent = XiaomanAgent::new(
        AgentDeps {
            chat: (llm)(config.chat.to_model_config(ModelRole::Chat)),
            operator: (llm)(config.operator.to_model_config(ModelRole::Operator)),
            device,
            memory: Arc::new(FileMemoryStore::new(config.memory_dir())),
            skills,
            sentinel: sentinel_trait,
            users,
            reminders,
        },
        AgentConfig {
            max_steps: config.max_steps,
            operator_notes: IosPlatform::operator_notes(),
            ..Default::default()
        },
    );
    let agent = match provider {
        Some(provider) => agent.with_external_tools(provider),
        None => agent,
    };

    Ok(RunnerSession {
        agent,
        sentinel,
        trace,
        skill_count: loaded.skills.len(),
    })
}

pub(crate) fn write_line(out: &Output, text: &str) {
    if let Ok(mut out) = out.lock() {
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
}
