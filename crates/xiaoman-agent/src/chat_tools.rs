//! Runs the chat model's tool calls. Argument and runtime errors become `{"error": ...}` results.

use std::sync::Arc;

use chrono::{Local, TimeZone};
use serde_json::{Map, Value, json};
use xiaoman_api::{
    AgentEvent, MemoryStore, Note, Observation, Relation, ReminderScheduler, Skill, SkillAction,
    SkillAuthoring, SkillChannel, SkillDraftAnswer, SkillDraftGateway, SkillFields,
    SkillRepository, ToolCall, ToolProvider, VocabAnswer, VocabCollocation, VocabEngine, VocabWord,
};

use crate::agent_tools;
use crate::json_text::JsonText;
use crate::note_text::NoteText;
use crate::reminder_times::ReminderTimes;

/// The authoring seam, only present where the user can confirm a draft.
#[derive(Clone)]
pub struct SkillEditing {
    pub authoring: Arc<dyn SkillAuthoring>,
    pub drafts: Arc<dyn SkillDraftGateway>,
}

pub struct ChatTools {
    pub memory: Arc<dyn MemoryStore>,
    pub skills: Arc<dyn SkillRepository>,
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub reminders: Arc<dyn ReminderScheduler>,
    pub external: Option<Arc<dyn ToolProvider>>,
    pub phone_tasks: bool,
    pub skill_editing: Option<SkillEditing>,
    pub vocab: Option<Arc<dyn VocabEngine>>,
    pub utc_offset_minutes: Option<i32>,
}

pub enum ToolError {
    Argument(String),
    Runtime(String),
}

impl ChatTools {
    pub async fn execute(
        &self,
        call: &ToolCall,
        task_id: &str,
        emit: &(dyn Fn(AgentEvent) + Send + Sync),
    ) -> String {
        match self.dispatch(call, task_id, emit).await {
            Ok(text) => text,
            Err(ToolError::Argument(message)) => error_json(&format!("参数错误：{message}")),
            Err(ToolError::Runtime(message)) => error_json(&format!("工具执行失败：{message}")),
        }
    }

    async fn dispatch(
        &self,
        call: &ToolCall,
        task_id: &str,
        emit: &(dyn Fn(AgentEvent) + Send + Sync),
    ) -> Result<String, ToolError> {
        let args = parse_args(&call.arguments_json)?;
        match call.name.as_str() {
            agent_tools::MEMORY_SEARCH => self.search(&args).await,
            agent_tools::MEMORY_READ => self.read(&args).await,
            agent_tools::MEMORY_WRITE => self.write(&args, task_id, emit).await,
            agent_tools::MEMORY_FORGET => self.forget(&args, task_id, emit).await,
            agent_tools::LIST_SKILLS => Ok(self.list_skills().await),
            agent_tools::SKILL_READ => self.skill_read(&args),
            agent_tools::SKILL_EDIT => self.skill_edit(&args).await,
            agent_tools::REMINDER_SET => self.reminder(&args),
            agent_tools::VOCAB_TODAY => self.vocab_today(),
            agent_tools::VOCAB_GRADE => self.vocab_grade(&args),
            agent_tools::VOCAB_STATUS => self.vocab_status(),
            agent_tools::VOCAB_PREFS => self.vocab_prefs(&args),
            agent_tools::VOCAB_ADD_WORD => self.vocab_add_word(&args),
            agent_tools::PHONE_TASK => {
                if self.phone_tasks {
                    Err(ToolError::Runtime("phone_task 由 agent 处理".to_string()))
                } else {
                    Err(ToolError::Runtime(
                        "这里不能操作手机，请用其他工具或告诉用户自己操作".to_string(),
                    ))
                }
            }
            _ => self.external(call, task_id, emit).await,
        }
    }

    async fn search(&self, args: &Map<String, Value>) -> Result<String, ToolError> {
        let query = required(args, "query")?;
        let limit = args
            .get("limit")
            .and_then(Value::as_i64)
            .unwrap_or(5)
            .clamp(1, 20) as usize;
        let hits = self
            .memory
            .search(&query, limit)
            .await
            .map_err(|error| ToolError::Runtime(error.0))?;
        let items: Vec<Value> = hits
            .into_iter()
            .map(|hit| json!({"permalink": hit.permalink, "title": hit.title, "snippet": hit.snippet}))
            .collect();
        Ok(json!({ "hits": items }).to_string())
    }

    async fn read(&self, args: &Map<String, Value>) -> Result<String, ToolError> {
        let identifier = required(args, "identifier")?;
        let note = self
            .memory
            .read(&identifier)
            .await
            .map_err(|error| ToolError::Runtime(error.0))?;
        let Some(note) = note else {
            return Ok(json!({"found": false}).to_string());
        };
        let observations: Vec<String> = note
            .observations
            .iter()
            .map(NoteText::observation_line)
            .collect();
        let relations: Vec<String> = note.relations.iter().map(NoteText::relation_line).collect();
        Ok(json!({
            "found": true,
            "title": note.title,
            "permalink": note.permalink,
            "type": note.note_type,
            "tags": note.tags,
            "observations": observations,
            "relations": relations,
            "body": note.body,
        })
        .to_string())
    }

    async fn write(
        &self,
        args: &Map<String, Value>,
        task_id: &str,
        emit: &(dyn Fn(AgentEvent) + Send + Sync),
    ) -> Result<String, ToolError> {
        let title = required(args, "title")?;
        let mut observations = Vec::new();
        for item in objects(args, "observations")? {
            observations.push(Observation {
                category: required(&item, "category")?,
                content: required(&item, "content")?,
                tags: JsonText::string_list(&item, "tags"),
                context: JsonText::string(&item, "context"),
            });
        }
        let mut relations = Vec::new();
        for item in objects(args, "relations")? {
            relations.push(Relation {
                relation_type: required(&item, "type")?,
                target: required(&item, "target")?,
                context: JsonText::string(&item, "context"),
            });
        }
        if observations.is_empty() && relations.is_empty() {
            return Err(ToolError::Argument("observations 不能为空".to_string()));
        }

        let identifier = JsonText::string(args, "permalink").unwrap_or_else(|| title.clone());
        let existing = self
            .memory
            .read(&identifier)
            .await
            .map_err(|error| ToolError::Runtime(error.0))?;
        let permalink = JsonText::string(args, "permalink")
            .or_else(|| existing.as_ref().map(|note| note.permalink.clone()))
            .unwrap_or_else(|| title.clone());

        let mut merged_observations: Vec<Observation> = existing
            .as_ref()
            .map(|note| note.observations.clone())
            .unwrap_or_default();
        for observation in observations {
            if !merged_observations.iter().any(|existing| {
                existing.category == observation.category && existing.content == observation.content
            }) {
                merged_observations.push(observation);
            }
        }
        let mut merged_relations: Vec<Relation> = existing
            .as_ref()
            .map(|note| note.relations.clone())
            .unwrap_or_default();
        for relation in relations {
            if !merged_relations.iter().any(|existing| {
                existing.relation_type == relation.relation_type
                    && existing.target == relation.target
            }) {
                merged_relations.push(relation);
            }
        }

        let preamble = existing
            .as_ref()
            .map(|note| NoteText::preamble(&note.body))
            .unwrap_or_default();
        let note = Note {
            title: title.clone(),
            permalink,
            note_type: JsonText::string(args, "type")
                .or_else(|| existing.as_ref().map(|note| note.note_type.clone()))
                .unwrap_or_else(|| "note".to_string()),
            tags: existing
                .as_ref()
                .map(|note| note.tags.clone())
                .unwrap_or_default(),
            extra_frontmatter: existing
                .as_ref()
                .map(|note| note.extra_frontmatter.clone())
                .unwrap_or_default(),
            body: NoteText::body(&title, &merged_observations, &merged_relations, &preamble),
            observations: merged_observations.clone(),
            relations: merged_relations,
            updated_at_millis: (self.clock)(),
        };
        let stored = self
            .memory
            .write(note)
            .await
            .map_err(|error| ToolError::Runtime(error.0))?;
        emit(AgentEvent::MemoryUpdated {
            task_id: task_id.to_string(),
            permalink: stored.permalink.clone(),
        });
        Ok(json!({
            "ok": true,
            "permalink": stored.permalink,
            "created": existing.is_none(),
            "observations": merged_observations.len(),
        })
        .to_string())
    }

    async fn forget(
        &self,
        args: &Map<String, Value>,
        task_id: &str,
        emit: &(dyn Fn(AgentEvent) + Send + Sync),
    ) -> Result<String, ToolError> {
        let identifier = required(args, "identifier")?;
        let permalink = self
            .memory
            .read(&identifier)
            .await
            .map_err(|error| ToolError::Runtime(error.0))?
            .map(|note| note.permalink)
            .unwrap_or_else(|| identifier.clone());
        let deleted = self
            .memory
            .delete(&identifier)
            .await
            .map_err(|error| ToolError::Runtime(error.0))?;
        if deleted {
            emit(AgentEvent::MemoryUpdated {
                task_id: task_id.to_string(),
                permalink,
            });
        }
        Ok(json!({"deleted": deleted}).to_string())
    }

    async fn list_skills(&self) -> String {
        let skills: Vec<Skill> = self.skills.list().await;
        let items: Vec<Value> = skills
            .iter()
            .map(|skill| {
                json!({
                    "name": skill.name,
                    "description": skill.description,
                    "channel": skill.channel.wire(),
                    "apps": skill.apps,
                    "hasFlow": skill.flow.is_some(),
                })
            })
            .collect();
        json!({"skills": items}).to_string()
    }

    fn reminder(&self, args: &Map<String, Value>) -> Result<String, ToolError> {
        let text = required(args, "text")?;
        let raw = required(args, "at")?;
        let at_millis = ReminderTimes::parse_at_millis(&raw).ok_or_else(|| {
            ToolError::Argument(
                "at 不是合法的 ISO-8601 本地时间，应为 2026-11-03T08:00 这样的形式".to_string(),
            )
        })?;
        if at_millis <= (self.clock)() {
            return Err(ToolError::Argument(format!(
                "提醒时间 {raw} 已经过去，请向用户确认一个将来的时间"
            )));
        }
        self.reminders.schedule(&text, at_millis);
        Ok(json!({"ok": true, "text": text, "at": raw}).to_string())
    }

    fn vocab_engine(&self) -> Result<&Arc<dyn VocabEngine>, ToolError> {
        self.vocab
            .as_ref()
            .ok_or_else(|| ToolError::Runtime("这里没有背单词引擎".to_string()))
    }

    fn local_today(&self) -> String {
        if let Some(minutes) = self.utc_offset_minutes {
            if let Some(offset) = chrono::FixedOffset::east_opt(minutes * 60) {
                if let Some(time) = chrono::Utc.timestamp_millis_opt((self.clock)()).single() {
                    return time
                        .with_timezone(&offset)
                        .date_naive()
                        .format("%Y-%m-%d")
                        .to_string();
                }
            }
        }
        Local
            .timestamp_millis_opt((self.clock)())
            .single()
            .unwrap_or_else(Local::now)
            .date_naive()
            .format("%Y-%m-%d")
            .to_string()
    }

    fn vocab_today(&self) -> Result<String, ToolError> {
        let engine = self.vocab_engine()?;
        let today = self.local_today();
        let plan = engine
            .today(&today)
            .map_err(|error| ToolError::Runtime(error.0))?;
        engine.ensure_reminders(
            (self.clock)(),
            self.utc_offset_minutes,
            self.reminders.as_ref(),
        );
        serde_json::to_string(&plan)
            .map_err(|error| ToolError::Runtime(format!("生成背单词计划失败：{error}")))
    }

    fn vocab_grade(&self, args: &Map<String, Value>) -> Result<String, ToolError> {
        let engine = self.vocab_engine()?;
        let items = objects(args, "answers")?;
        if items.is_empty() {
            return Err(ToolError::Argument("answers 不能为空".to_string()));
        }
        let mut answers = Vec::with_capacity(items.len());
        for item in items {
            let word = required(&item, "word")?;
            let correct = JsonText::bool_of(&item, "correct").ok_or_else(|| {
                ToolError::Argument(format!("{word} 的 correct 必须是 true 或 false"))
            })?;
            answers.push(VocabAnswer { word, correct });
        }
        let outcome = engine
            .grade(&self.local_today(), &answers)
            .map_err(|error| ToolError::Runtime(error.0))?;
        serde_json::to_string(&outcome)
            .map_err(|error| ToolError::Runtime(format!("生成复习结果失败：{error}")))
    }

    fn vocab_status(&self) -> Result<String, ToolError> {
        let engine = self.vocab_engine()?;
        let today = self.local_today();
        let stats = engine
            .stats(&today)
            .map_err(|error| ToolError::Runtime(error.0))?;
        let prefs = engine
            .prefs()
            .map_err(|error| ToolError::Runtime(error.0))?;
        Ok(json!({"date": today, "stats": stats, "prefs": prefs}).to_string())
    }

    fn vocab_prefs(&self, args: &Map<String, Value>) -> Result<String, ToolError> {
        let engine = self.vocab_engine()?;
        let mut prefs = engine
            .prefs()
            .map_err(|error| ToolError::Runtime(error.0))?;
        let mut changed = false;
        if let Some(value) = args.get("daily_new") {
            let number = value
                .as_u64()
                .ok_or_else(|| ToolError::Argument("daily_new 必须是整数".to_string()))?;
            prefs.daily_new = u32::try_from(number)
                .map_err(|_| ToolError::Argument("daily_new 超出范围".to_string()))?;
            changed = true;
        }
        if let Some(time) = JsonText::string(args, "reminder_time") {
            prefs.reminder_time = time;
            changed = true;
        }
        if let Some(enabled) = JsonText::bool_of(args, "reminder_enabled") {
            prefs.reminder_enabled = enabled;
            changed = true;
        }
        if changed {
            prefs = engine
                .set_prefs(prefs)
                .map_err(|error| ToolError::Runtime(error.0))?;
        }
        engine.ensure_reminders(
            (self.clock)(),
            self.utc_offset_minutes,
            self.reminders.as_ref(),
        );
        serde_json::to_string(&prefs)
            .map_err(|error| ToolError::Runtime(format!("生成偏好失败：{error}")))
    }

    fn vocab_add_word(&self, args: &Map<String, Value>) -> Result<String, ToolError> {
        let engine = self.vocab_engine()?;
        let word = required(args, "word")?;
        let meaning = required(args, "meaning")?;
        let mut collocations = Vec::new();
        for item in objects(args, "collocations")? {
            collocations.push(VocabCollocation {
                en: required(&item, "en")?,
                zh: required(&item, "zh")?,
            });
        }
        let card = VocabWord {
            word,
            meaning,
            phonetic: JsonText::string(args, "phonetic").unwrap_or_default(),
            pos: JsonText::string(args, "pos").unwrap_or_default(),
            example: JsonText::string(args, "example").unwrap_or_default(),
            example_zh: JsonText::string(args, "example_zh").unwrap_or_default(),
            collocations,
        };
        let added = engine
            .add_word(&self.local_today(), card)
            .map_err(|error| ToolError::Runtime(error.0))?;
        Ok(json!({
            "ok": true,
            "word": added.word,
            "phonetic": added.phonetic,
            "meaning": added.meaning,
        })
        .to_string())
    }

    async fn external(
        &self,
        call: &ToolCall,
        task_id: &str,
        emit: &(dyn Fn(AgentEvent) + Send + Sync),
    ) -> Result<String, ToolError> {
        let Some(provider) = &self.external else {
            return Err(ToolError::Runtime(format!("未知工具 {}", call.name)));
        };
        let Some(result) = provider
            .call(task_id, &call.name, &call.arguments_json)
            .await
        else {
            return Err(ToolError::Runtime(format!("未知工具 {}", call.name)));
        };
        emit(AgentEvent::ToolCalled {
            task_id: task_id.to_string(),
            tool: call.name.clone(),
            ok: result.ok,
            summary: result.summary.clone(),
        });
        Ok(result.text)
    }

    fn skill_read(&self, args: &Map<String, Value>) -> Result<String, ToolError> {
        let editing = self
            .skill_editing
            .as_ref()
            .ok_or_else(|| ToolError::Runtime("这里不支持编辑技能".to_string()))?;
        let name = required(args, "name")?;
        match editing.authoring.read(&name) {
            Some(result) => Ok(json!({
                "found": true,
                "name": result.name,
                "content": result.content,
                "bundled": result.bundled,
            })
            .to_string()),
            None => Ok(json!({"found": false, "name": name}).to_string()),
        }
    }

    async fn skill_edit(&self, args: &Map<String, Value>) -> Result<String, ToolError> {
        let editing = self
            .skill_editing
            .as_ref()
            .ok_or_else(|| ToolError::Runtime("这里不支持编辑技能".to_string()))?;
        let action = match required(args, "action")?.as_str() {
            "create" => SkillAction::Create,
            "update" => SkillAction::Update,
            "delete" => SkillAction::Delete,
            other => {
                return Err(ToolError::Argument(format!(
                    "action 只能是 create / update / delete，收到 {other}"
                )));
            }
        };
        let name = required(args, "name")?;
        let draft = if action == SkillAction::Delete {
            editing.authoring.prepare_delete(&name)
        } else {
            let wire = required(args, "channel")?;
            let channel = SkillChannel::from_wire(&wire).ok_or_else(|| {
                ToolError::Argument(format!(
                    "channel 只能是 api/cli/deeplink/gui/deeplink-then-gui/mixed/chat，收到 {wire}"
                ))
            })?;
            let fields = SkillFields {
                name,
                description: required(args, "description")?,
                channel,
                instructions: required(args, "instructions")?,
                apps: JsonText::string_list(args, "apps"),
                confirm_before: JsonText::string_list(args, "confirm_before"),
                memory_refs: JsonText::string_list(args, "memory"),
                deeplinks: deeplinks(args)?,
            };
            editing.authoring.prepare_write(action, fields)
        }
        .map_err(ToolError::Runtime)?;

        let answer = editing
            .drafts
            .request(draft.clone())
            .await
            .map_err(ToolError::Runtime)?;
        if answer == SkillDraftAnswer::Cancel {
            return Ok(json!({"ok": false, "cancelled": true}).to_string());
        }
        editing
            .authoring
            .commit(&draft)
            .map_err(ToolError::Runtime)?;
        let reloaded = self.skills.reload().await;
        Ok(json!({
            "ok": true,
            "action": match draft.action {
                SkillAction::Create => "create",
                SkillAction::Update => "update",
                SkillAction::Delete => "delete",
            },
            "name": draft.name,
            "skills": reloaded.skills.len(),
        })
        .to_string())
    }
}

fn deeplinks(args: &Map<String, Value>) -> Result<Vec<(String, String)>, ToolError> {
    match args.get("deeplinks") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|text| (key.clone(), text.to_string()))
                    .ok_or_else(|| ToolError::Argument(format!("deeplinks.{key} 必须是字符串")))
            })
            .collect(),
        Some(_) => Err(ToolError::Argument("deeplinks 必须是对象".to_string())),
    }
}

pub fn phone_result_json(event: &AgentEvent) -> String {
    match event {
        AgentEvent::PhoneTaskFinished {
            success,
            summary,
            steps,
            ..
        } => json!({"success": success, "summary": summary, "steps": steps}).to_string(),
        other => error_json(&format!("phone_task 未返回结果：{}", other.kind())),
    }
}

pub fn parse_args(raw: &str) -> Result<Map<String, Value>, ToolError> {
    if raw.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(ToolError::Argument(
            "arguments 不是合法的 JSON 对象".to_string(),
        )),
    }
}

fn required(args: &Map<String, Value>, key: &str) -> Result<String, ToolError> {
    JsonText::string(args, key).ok_or_else(|| ToolError::Argument(format!("缺少 {key}")))
}

fn objects(args: &Map<String, Value>, key: &str) -> Result<Vec<Map<String, Value>>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Object(map) => Ok(map.clone()),
                _ => Err(ToolError::Argument(format!("{key} 的每一项必须是对象"))),
            })
            .collect(),
        Some(Value::Object(map)) => Ok(vec![map.clone()]),
        _ => Err(ToolError::Argument(format!("{key} 必须是数组"))),
    }
}

fn error_json(message: &str) -> String {
    json!({"error": message}).to_string()
}
