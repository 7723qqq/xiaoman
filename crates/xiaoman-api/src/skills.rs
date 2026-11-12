//! Scenario skills: a folder with `SKILL.md` and an optional `flow.yaml`.

use async_trait::async_trait;

/// A scenario skill written by users.
///
/// `SKILL.md` frontmatter:
///
/// ```text
/// ---
/// name: taxi-home                 # required, [a-z0-9-], unique
/// description: 打车回家            # required, one line; shown to the chat model
/// apps: [com.sdu.didi.psnger]     # package names the skill may operate
/// channel: deeplink-then-gui      # api | cli | deeplink | gui | deeplink-then-gui | mixed | chat
/// confirm_before: [确认呼叫, 支付]  # words that force a confirmation before tapping or typing on them
/// memory: [地址/家, 偏好/车型]       # notes to load into the operator or chat prompt
/// deeplinks:
///   search: "amapuri://route/plan/?dname={destination}"
/// ---
/// ```
///
/// Channel `chat` runs the Markdown body in the conversation; other channels serve phone tasks.
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub apps: Vec<String>,
    pub channel: SkillChannel,
    pub confirm_before: Vec<String>,
    pub memory_refs: Vec<String>,
    pub deeplinks: Vec<(String, String)>,
    /// Markdown body of SKILL.md after the frontmatter.
    pub instructions: String,
    pub flow: Option<Flow>,
    /// Where the skill was loaded from (folder path or URI), for display and reload.
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillChannel {
    Api,
    Cli,
    Deeplink,
    Gui,
    DeeplinkThenGui,
    Mixed,
    Chat,
}

impl SkillChannel {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Cli => "cli",
            Self::Deeplink => "deeplink",
            Self::Gui => "gui",
            Self::DeeplinkThenGui => "deeplink-then-gui",
            Self::Mixed => "mixed",
            Self::Chat => "chat",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        let value = value.trim().to_lowercase();
        [
            Self::Api,
            Self::Cli,
            Self::Deeplink,
            Self::Gui,
            Self::DeeplinkThenGui,
            Self::Mixed,
            Self::Chat,
        ]
        .into_iter()
        .find(|channel| channel.wire() == value)
    }
}

/// Optional `flow.yaml`: fixed steps run before or between free operator steps.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Flow {
    pub steps: Vec<FlowStep>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FlowStep {
    Launch { package_name: String },
    Deeplink { key_or_uri: String },
    Ai { goal: String },
    Confirm { question: String },
    Tap { target: String },
    Type { target: String, text: String },
    Wait { millis: i64 },
    Assert { expectation: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillProblem {
    pub source: String,
    pub message: String,
    pub line: Option<usize>,
}

/// Result of loading skills: problems never abort loading of the other skills.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SkillLoadResult {
    pub skills: Vec<Skill>,
    pub problems: Vec<SkillProblem>,
}

#[async_trait]
pub trait SkillRepository: Send + Sync {
    async fn reload(&self) -> SkillLoadResult;

    async fn list(&self) -> Vec<Skill>;

    async fn get(&self, name: &str) -> Option<Skill>;
}
