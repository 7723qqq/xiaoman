//! Skill authoring: the draft the chat model proposes and the user confirms.

use async_trait::async_trait;

use crate::skills::SkillChannel;

/// What the model asks the authoring layer to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillAction {
    Create,
    Update,
    Delete,
}

/// The structured fields of `SKILL.md`. The model never submits raw YAML.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillFields {
    pub name: String,
    pub description: String,
    pub channel: SkillChannel,
    pub instructions: String,
    pub apps: Vec<String>,
    pub confirm_before: Vec<String>,
    pub memory_refs: Vec<String>,
    pub deeplinks: Vec<(String, String)>,
}

impl SkillFields {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        channel: SkillChannel,
        instructions: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            channel,
            instructions: instructions.into(),
            apps: Vec::new(),
            confirm_before: Vec::new(),
            memory_refs: Vec::new(),
            deeplinks: Vec::new(),
        }
    }
}

/// One rendered skill waiting for the user's yes.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillDraft {
    pub action: SkillAction,
    pub name: String,
    /// Rendered `SKILL.md`; empty for [`SkillAction::Delete`].
    pub content: String,
    /// Current user or bundled `SKILL.md`, when the skill already exists.
    pub previous: Option<String>,
    /// A bundled skill with this name exists: an update creates a user copy.
    pub shadows_bundled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillDraftAnswer {
    Save,
    Cancel,
}

/// The UI side of a draft: parks it until the user saves or cancels.
#[async_trait]
pub trait SkillDraftGateway: Send + Sync {
    async fn request(&self, draft: SkillDraft) -> Result<SkillDraftAnswer, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillReadResult {
    pub name: String,
    pub content: String,
    /// A bundled skill with this name exists (whether or not a user copy shadows it).
    pub bundled: bool,
}

/// Renders, validates and commits skill folders. Only `commit` writes anything.
pub trait SkillAuthoring: Send + Sync {
    fn read(&self, name: &str) -> Option<SkillReadResult>;

    /// `action` must be [`SkillAction::Create`] or [`SkillAction::Update`].
    fn prepare_write(&self, action: SkillAction, fields: SkillFields)
    -> Result<SkillDraft, String>;

    fn prepare_delete(&self, name: &str) -> Result<SkillDraft, String>;

    /// Create/Update write `<user>/<name>/SKILL.md`; Delete removes the user folder.
    fn commit(&self, draft: &SkillDraft) -> Result<(), String>;
}
