//! The skills screen's rows: `SkillsScreen.kt` over `xiaoman_api::{Skill, SkillProblem}`.
//!
//! `SkillsViewModel` reloads through a coroutine; here the caller owns that and this module maps
//! the load result to the cards and problem rows the screen paints.

use xiaoman_api::{Skill, SkillLoadResult, SkillProblem};

use super::copy;

/// The flow indicator of a skill: `Flow` steps run before or between free operator steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowRow {
    pub steps: usize,
    pub label: String,
}

impl FlowRow {
    pub fn from_skill(skill: &Skill) -> Option<Self> {
        let steps = skill
            .flow
            .as_ref()
            .map(|flow| flow.steps.len())
            .unwrap_or(0);
        (steps > 0).then(|| Self {
            steps,
            label: copy::skills_flow(steps),
        })
    }
}

/// One skill card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRow {
    pub name: String,
    pub description: String,
    /// `App：com.a, com.b`, or `None` when the skill names no apps.
    pub apps: Option<String>,
    pub source: String,
    /// `流程 N 步`, or `None` for a skill without a `flow.yaml`.
    pub flow: Option<FlowRow>,
}

/// One failed skill source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemRow {
    /// `source` plus `:line` when the loader knows the line.
    pub source: String,
    pub message: String,
}

impl ProblemRow {
    pub fn from_problem(problem: &SkillProblem) -> Self {
        let source = match problem.line {
            Some(line) => format!("{}:{}", problem.source, line),
            None => problem.source.clone(),
        };
        Self {
            source,
            message: problem.message.clone(),
        }
    }
}

/// The skills screen's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillsModel {
    rows: Vec<SkillRow>,
    problems: Vec<ProblemRow>,
    loading: bool,
    skills_dir: String,
}

impl Default for SkillsModel {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            problems: Vec::new(),
            loading: true,
            skills_dir: String::new(),
        }
    }
}

impl SkillsModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_folder(mut self, folder: impl Into<String>) -> Self {
        self.skills_dir = folder.into();
        self
    }

    pub fn rows(&self) -> &[SkillRow] {
        &self.rows
    }

    pub fn problems(&self) -> &[ProblemRow] {
        &self.problems
    }

    pub fn loading(&self) -> bool {
        self.loading
    }

    pub fn reload(&mut self) {
        self.loading = true;
    }

    /// `repository.reload()`: the cards, the problems, and the end of the progress bar.
    pub fn set_result(&mut self, result: &SkillLoadResult) {
        self.rows = skill_rows(&result.skills);
        self.problems = result
            .problems
            .iter()
            .map(ProblemRow::from_problem)
            .collect();
        self.loading = false;
    }

    /// A reload that failed outright: no skills, one problem.
    pub fn set_failed(&mut self, message: impl Into<String>) {
        self.rows.clear();
        self.problems = vec![ProblemRow {
            source: self.skills_dir.clone(),
            message: message.into(),
        }];
        self.loading = false;
    }

    /// `skills_empty`: only while idle with nothing loaded.
    pub fn empty_hint(&self) -> Option<String> {
        (self.rows.is_empty() && !self.loading).then(|| copy::skills_empty(&self.skills_dir))
    }
}

pub fn skill_rows(skills: &[Skill]) -> Vec<SkillRow> {
    skills
        .iter()
        .map(|skill| SkillRow {
            name: skill.name.clone(),
            description: skill.description.clone(),
            apps: (!skill.apps.is_empty()).then(|| copy::skills_apps(&skill.apps.join(", "))),
            source: skill.source.clone(),
            flow: FlowRow::from_skill(skill),
        })
        .collect()
}
