//! Loads skills from folders `<root>/<name>/SKILL.md` (+ optional `flow.yaml`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use xiaoman_api::{Skill, SkillLoadResult, SkillProblem, SkillRepository};

use crate::skill_parser::SkillParser;

/// Later roots override earlier ones by skill name, so pass bundled skills first and user skills
/// last. Missing roots are skipped silently. A broken skill is reported in
/// [`SkillLoadResult::problems`] and never stops the others from loading; when a user skill is
/// broken, a bundled skill of the same name stays in place.
pub struct DirectorySkillRepository {
    roots: Vec<PathBuf>,
    state: Mutex<Option<State>>,
}

#[derive(Clone)]
struct State {
    result: SkillLoadResult,
    by_name: HashMap<String, Skill>,
}

impl DirectorySkillRepository {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            state: Mutex::new(None),
        }
    }

    /// Loads one root without overrides; duplicate skill names inside it are problems.
    pub fn load_root(root: &Path) -> SkillLoadResult {
        if !root.exists() {
            return SkillLoadResult::default();
        }
        let Ok(dir) = fs::read_dir(root) else {
            return SkillLoadResult {
                skills: Vec::new(),
                problems: vec![SkillProblem {
                    source: root.to_string_lossy().to_string(),
                    message: "not a readable directory".to_string(),
                    line: None,
                }],
            };
        };
        let mut folders: Vec<PathBuf> = dir
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .filter(|path| {
                path.file_name()
                    .map(|name| !name.to_string_lossy().starts_with('.'))
                    .unwrap_or(false)
            })
            .collect();
        folders.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

        let mut parsed: Vec<(PathBuf, Skill)> = Vec::new();
        let mut problems: Vec<SkillProblem> = Vec::new();
        for folder in folders {
            let skill_file = folder.join("SKILL.md");
            if !skill_file.is_file() {
                problems.push(SkillProblem {
                    source: folder.to_string_lossy().to_string(),
                    message: "missing SKILL.md".to_string(),
                    line: None,
                });
                continue;
            }
            let flow_file = folder.join("flow.yaml");
            let folder_name = folder
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            let skill_md = fs::read_to_string(&skill_file);
            let flow_yaml = if flow_file.is_file() {
                fs::read_to_string(&flow_file).ok()
            } else {
                None
            };
            let (skill, skill_problems) = match skill_md {
                Ok(text) => SkillParser::parse_folder(
                    &folder_name,
                    &text,
                    flow_yaml.as_deref(),
                    &folder.to_string_lossy(),
                ),
                Err(error) => (
                    None,
                    vec![SkillProblem {
                        source: folder.to_string_lossy().to_string(),
                        message: format!("cannot read: {error}"),
                        line: None,
                    }],
                ),
            };
            problems.extend(skill_problems);
            if let Some(skill) = skill {
                parsed.push((folder, skill));
            }
        }

        let mut order: Vec<String> = Vec::new();
        let mut grouped: HashMap<String, Vec<(PathBuf, Skill)>> = HashMap::new();
        for (folder, skill) in parsed {
            if !grouped.contains_key(&skill.name) {
                order.push(skill.name.clone());
            }
            grouped
                .entry(skill.name.clone())
                .or_default()
                .push((folder, skill));
        }
        let mut skills = Vec::new();
        for name in order {
            let candidates = grouped.remove(&name).unwrap_or_default();
            let keep = candidates
                .iter()
                .position(|(folder, _)| {
                    folder
                        .file_name()
                        .map(|file| file.to_string_lossy() == name)
                        .unwrap_or(false)
                })
                .unwrap_or(0);
            let kept = candidates[keep].clone();
            for (index, (folder, _)) in candidates.iter().enumerate() {
                if index == keep {
                    continue;
                }
                problems.push(SkillProblem {
                    source: folder.to_string_lossy().to_string(),
                    message: format!(
                        "duplicate skill name '{name}' (also in {}); ignored",
                        kept.0.to_string_lossy()
                    ),
                    line: None,
                });
            }
            skills.push(kept.1);
        }
        SkillLoadResult { skills, problems }
    }

    fn load(&self) -> State {
        let mut merged: Vec<Skill> = Vec::new();
        let mut problems = Vec::new();
        for root in &self.roots {
            let result = Self::load_root(root);
            problems.extend(result.problems);
            for skill in result.skills {
                merged.retain(|existing: &Skill| existing.name != skill.name);
                merged.push(skill);
            }
        }
        merged.sort_by(|a, b| a.name.cmp(&b.name));
        let by_name = merged
            .iter()
            .map(|skill| (skill.name.clone(), skill.clone()))
            .collect();
        State {
            result: SkillLoadResult {
                skills: merged,
                problems,
            },
            by_name,
        }
    }

    fn snapshot(&self) -> State {
        {
            let mut guard = self.state.lock().unwrap();
            if guard.is_none() {
                *guard = Some(self.load());
            }
        }
        self.state
            .lock()
            .unwrap()
            .as_ref()
            .cloned()
            .expect("state loaded")
    }
}

#[async_trait]
impl SkillRepository for DirectorySkillRepository {
    async fn reload(&self) -> SkillLoadResult {
        let state = self.load();
        let result = state.result.clone();
        *self.state.lock().unwrap() = Some(state);
        result
    }

    async fn list(&self) -> Vec<Skill> {
        self.snapshot().result.skills
    }

    async fn get(&self, name: &str) -> Option<Skill> {
        self.snapshot().by_name.get(name).cloned()
    }
}
