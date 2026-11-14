//! State persistence: a JSON file on disk, or memory for tests and the web session.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use xiaoman_api::VocabError;

use crate::model::VocabState;

pub trait VocabStore: Send + Sync {
    /// `Ok(None)` means there is no state yet; a corrupt file is moved aside and treated as new.
    fn load(&self) -> Result<Option<VocabState>, VocabError>;
    fn save(&self, state: &VocabState) -> Result<(), VocabError>;
}

pub struct FileVocabStore {
    path: PathBuf,
}

impl FileVocabStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load_file(&self, path: &Path) -> Result<Option<VocabState>, VocabError> {
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(path).map_err(|error| {
            VocabError(format!("读取背单词状态失败（{}）：{error}", path.display()))
        })?;
        match serde_json::from_str::<VocabState>(&text) {
            Ok(state) => Ok(Some(state)),
            Err(error) => {
                let backup =
                    path.with_extension(format!("corrupt-{}.json", xiaoman_api::now_millis()));
                let _ = fs::rename(path, &backup);
                eprintln!(
                    "vocab: {} 无法解析，已备份到 {}：{error}",
                    path.display(),
                    backup.display()
                );
                Ok(None)
            }
        }
    }
}

impl VocabStore for FileVocabStore {
    fn load(&self) -> Result<Option<VocabState>, VocabError> {
        match self.load_file(&self.path)? {
            Some(state) => Ok(Some(state)),
            // A crash between writing the temp file and replacing the state leaves a usable copy.
            None => {
                let temp = self.path.with_extension("json.tmp");
                if let Some(state) = self.load_file(&temp)? {
                    return Ok(Some(state));
                }
                self.load_file(&self.path.with_extension("json.bak"))
            }
        }
    }

    fn save(&self, state: &VocabState) -> Result<(), VocabError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                VocabError(format!(
                    "创建背单词目录失败（{}）：{error}",
                    parent.display()
                ))
            })?;
        }
        let json = serde_json::to_vec_pretty(state)
            .map_err(|error| VocabError(format!("序列化背单词状态失败：{error}")))?;
        let temp = self.path.with_extension("json.tmp");
        fs::write(&temp, json).map_err(|error| {
            VocabError(format!("写入背单词状态失败（{}）：{error}", temp.display()))
        })?;
        let backup = self.path.with_extension("json.bak");
        if self.path.exists() {
            let _ = fs::remove_file(&backup);
            if let Err(error) = fs::rename(&self.path, &backup) {
                let _ = fs::remove_file(&temp);
                return Err(VocabError(format!(
                    "备份背单词状态失败（{}）：{error}",
                    self.path.display()
                )));
            }
        }
        match fs::rename(&temp, &self.path) {
            Ok(()) => {
                let _ = fs::remove_file(&backup);
                Ok(())
            }
            Err(error) => {
                let _ = fs::rename(&backup, &self.path);
                let _ = fs::remove_file(&temp);
                Err(VocabError(format!(
                    "替换背单词状态失败（{}）：{error}",
                    self.path.display()
                )))
            }
        }
    }
}

#[derive(Default)]
pub struct MemoryVocabStore {
    state: Mutex<Option<VocabState>>,
}

impl MemoryVocabStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The stored state, for tests and the web page.
    pub fn snapshot(&self) -> Option<VocabState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl VocabStore for MemoryVocabStore {
    fn load(&self) -> Result<Option<VocabState>, VocabError> {
        Ok(self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone())
    }

    fn save(&self, state: &VocabState) -> Result<(), VocabError> {
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(state.clone());
        Ok(())
    }
}
