//! Runs the chat model's tool calls. Argument and runtime errors become `{"error": ...}` results.

use std::sync::Arc;

use serde_json::{Map, Value, json};
use xiaoman_api::{
    AgentEvent, MemoryStore, Note, Observation, Relation, ReminderScheduler, Skill,
    SkillRepository, ToolCall, ToolProvider,
};

use crate::agent_tools;
use crate::json_text::JsonText;
use crate::note_text::NoteText;
use crate::reminder_times::ReminderTimes;

pub struct ChatTools {
    pub memory: Arc<dyn MemoryStore>,
    pub skills: Arc<dyn SkillRepository>,
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub reminders: Arc<dyn ReminderScheduler>,
    pub external: Option<Arc<dyn ToolProvider>>,
    pub phone_tasks: bool,
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
            agent_tools::REMINDER_SET => self.reminder(&args),
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
