//! The Xiaoman agent: chat turns with memory and phone tools (`handle`), and phone tasks run by the
//! operator loop or a skill flow (`run_phone_task`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoman_api::{
    Agent, AgentConfig, AgentEvent, AgentListener, ChatMessage, ChatRequest, ChatRole, ContentPart,
    Device, LlmClient, MemoryStore, Note, Observation, ReminderScheduler, Sentinel, Skill,
    SkillChannel, SkillRepository, StopSignal, ToolCall, ToolProvider, ToolSpec, UserChannel,
};

use crate::agent_tools;
use crate::chat_tools::{ChatTools, phone_result_json};
use crate::json_text::JsonText;
use crate::note_text::NoteText;
use crate::operator_loop::OperatorLoop;
use crate::prompts;
use crate::reminder_times::ReminderTimes;
use crate::task_run::TaskRun;

pub const MAX_TOOL_ROUNDS: i32 = 8;
pub const HISTORY_LIMIT: usize = 20;
pub const MEMORY_HITS: usize = 5;
pub const HOWTO_TYPE: &str = "howto";
pub const PATH_CATEGORY: &str = "path";
const MAX_HOWTO_NOTES: usize = 3;
pub const MAX_CHAT_SKILL_CHARS: usize = 2000;
pub const MAX_CHAT_SKILLS_CHARS: usize = 8000;

pub struct AgentDeps {
    pub chat: Arc<dyn LlmClient>,
    pub operator: Arc<dyn LlmClient>,
    pub device: Arc<dyn Device>,
    pub memory: Arc<dyn MemoryStore>,
    pub skills: Arc<dyn SkillRepository>,
    pub sentinel: Arc<dyn Sentinel>,
    pub users: Arc<dyn UserChannel>,
    pub reminders: Arc<dyn ReminderScheduler>,
}

pub struct XiaomanAgent {
    chat: Arc<dyn LlmClient>,
    memory: Arc<dyn MemoryStore>,
    skills: Arc<dyn SkillRepository>,
    users: Arc<dyn UserChannel>,
    reminders: Arc<dyn ReminderScheduler>,
    config: AgentConfig,
    persona: String,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    id_factory: Arc<dyn Fn() -> String + Send + Sync>,
    external_tools: Option<Arc<dyn ToolProvider>>,
    operator_loop: OperatorLoop,
    history: Mutex<VecDeque<ChatMessage>>,
}

impl XiaomanAgent {
    pub fn new(deps: AgentDeps, config: AgentConfig) -> Self {
        let operator_loop = OperatorLoop::new(
            deps.operator,
            deps.device,
            deps.sentinel,
            deps.users.clone(),
            config.clone(),
        );
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let id_factory: Arc<dyn Fn() -> String + Send + Sync> = Arc::new(move || {
            format!(
                "{}-{}",
                xiaoman_api::now_millis(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            )
        });
        Self {
            chat: deps.chat,
            memory: deps.memory,
            skills: deps.skills,
            users: deps.users,
            reminders: deps.reminders,
            config,
            persona: prompts::DEFAULT_PERSONA.to_string(),
            clock: Arc::new(xiaoman_api::now_millis),
            id_factory,
            external_tools: None,
            operator_loop,
            history: Mutex::new(VecDeque::new()),
        }
    }

    pub fn with_persona(mut self, persona: impl Into<String>) -> Self {
        self.persona = persona.into();
        self
    }

    pub fn with_external_tools(mut self, provider: Arc<dyn ToolProvider>) -> Self {
        self.external_tools = Some(provider);
        self
    }

    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_id_factory(mut self, factory: Arc<dyn Fn() -> String + Send + Sync>) -> Self {
        self.id_factory = factory;
        self
    }

    fn built_in_specs(&self) -> Vec<ToolSpec> {
        let specs = agent_tools::specs();
        if self.config.phone_tasks {
            specs
        } else {
            specs
                .into_iter()
                .filter(|spec| spec.name != agent_tools::PHONE_TASK)
                .collect()
        }
    }

    async fn external_specs(&self) -> Vec<ToolSpec> {
        let Some(provider) = &self.external_tools else {
            return Vec::new();
        };
        let specs = agent_tools::specs();
        let built_in: Vec<String> = specs.iter().map(|spec| spec.name.clone()).collect();
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for spec in provider.specs().await {
            if built_in.contains(&spec.name) || seen.contains(&spec.name) {
                continue;
            }
            seen.push(spec.name.clone());
            out.push(spec);
        }
        out
    }

    async fn system_prompt(&self, user_message: &str, has_external_tools: bool) -> String {
        let mut out = self.persona.trim().to_string();
        if has_external_tools {
            out.push('\n');
            out.push_str(prompts::EXTERNAL_TOOLS_HINT);
        }
        out.push_str("\n\n");
        out.push_str(&prompts::now_section(&ReminderTimes::prompt_now((self
            .clock)(
        ))));
        let list = self.skills.list().await;
        if self.config.phone_tasks {
            out.push_str("\n\n");
            out.push_str(prompts::SKILLS_HEADER);
            out.push('\n');
            let phone_skills: Vec<&Skill> = list
                .iter()
                .filter(|skill| skill.channel != SkillChannel::Chat)
                .collect();
            if phone_skills.is_empty() {
                out.push_str(prompts::NO_SKILLS);
            } else {
                for skill in phone_skills {
                    out.push_str(&format!("- {}: {}\n", skill.name, skill.description));
                }
            }
        }
        let chat_skills: Vec<&Skill> = list
            .iter()
            .filter(|skill| skill.channel == SkillChannel::Chat)
            .collect();
        if !chat_skills.is_empty() {
            out.push_str("\n\n");
            out.push_str(prompts::CHAT_SKILLS_HEADER);
            out.push('\n');
            out.push_str(prompts::CHAT_SKILLS_HINT);
            let mut remaining = MAX_CHAT_SKILLS_CHARS;
            for skill in chat_skills {
                out.push_str(&format!("\n\n### {}：{}", skill.name, skill.description));
                if remaining == 0 {
                    continue;
                }
                let instructions: String = skill
                    .instructions
                    .chars()
                    .take(MAX_CHAT_SKILL_CHARS)
                    .take(remaining)
                    .collect();
                if !instructions.is_empty() {
                    out.push('\n');
                    out.push_str(&instructions);
                }
                remaining = remaining.saturating_sub(instructions.chars().count());
                for reference in &skill.memory_refs {
                    if remaining == 0 {
                        break;
                    }
                    let Ok(Some(note)) = self.memory.read(reference).await else {
                        continue;
                    };
                    let text: String = NoteText::for_prompt(&note, 1500)
                        .chars()
                        .take(remaining)
                        .collect();
                    out.push_str("\n\n");
                    out.push_str(&text);
                    remaining = remaining.saturating_sub(text.chars().count());
                }
            }
        }
        let hits = self
            .memory
            .search(user_message, MEMORY_HITS)
            .await
            .unwrap_or_default();
        if !hits.is_empty() {
            out.push_str("\n\n");
            out.push_str(prompts::MEMORY_HEADER);
            out.push('\n');
            for hit in hits {
                out.push_str(&format!(
                    "- {} ({}): {}\n",
                    hit.title,
                    hit.permalink,
                    hit.snippet.replace('\n', " ")
                ));
            }
        }
        out.trim_end().to_string()
    }

    async fn remember(&self, user_message: &str, reply: Option<String>) {
        let mut history = self.history.lock().unwrap();
        history.push_back(ChatMessage::user(user_message));
        if let Some(reply) = reply.filter(|reply| !reply.trim().is_empty()) {
            history.push_back(ChatMessage::assistant(reply));
        }
        while history.len() > HISTORY_LIMIT {
            history.pop_front();
        }
        while history.front().map(|message| message.role) == Some(ChatRole::Assistant) {
            history.pop_front();
        }
    }

    async fn references(&self, skill: Option<&Skill>) -> String {
        let Some(skill) = skill else {
            return String::new();
        };
        let mut parts = Vec::new();
        for reference in &skill.memory_refs {
            if let Ok(Some(note)) = self.memory.read(reference).await {
                parts.push(NoteText::for_prompt(&note, 1500));
            }
        }
        parts.join("\n\n")
    }

    /// `howto` notes for the goal: the skill's own note first, then search hits of type howto.
    async fn howto(&self, goal: &str, skill: Option<&Skill>) -> String {
        let mut notes: Vec<(String, Note)> = Vec::new();
        let push = |notes: &mut Vec<(String, Note)>, note: Note| {
            if !notes
                .iter()
                .any(|(permalink, _)| permalink == &note.permalink)
            {
                notes.push((note.permalink.clone(), note));
            }
        };
        if let Some(skill) = skill {
            if let Ok(Some(note)) = self.memory.read(&howto_permalink(&skill.name)).await {
                push(&mut notes, note);
            }
        }
        for hit in self
            .memory
            .search(goal, MEMORY_HITS)
            .await
            .unwrap_or_default()
        {
            if notes.len() >= MAX_HOWTO_NOTES {
                break;
            }
            if notes
                .iter()
                .any(|(permalink, _)| permalink == &hit.permalink)
            {
                continue;
            }
            if let Ok(Some(note)) = self.memory.read(&hit.permalink).await {
                if note.note_type == HOWTO_TYPE {
                    push(&mut notes, note);
                }
            }
        }
        notes
            .into_iter()
            .take(MAX_HOWTO_NOTES)
            .map(|(_, note)| NoteText::for_prompt(&note, 800))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Records the successful path as `- [path] a → b → c #skill` in `howto/<skill-or-app>`.
    async fn learn(&self, task: &TaskRun) {
        if task.path.is_empty() {
            return;
        }
        let Some(key) = task
            .skill
            .as_ref()
            .map(|skill| skill.name.clone())
            .or_else(|| task.launched_package.clone())
            .or_else(|| task.last_package.clone())
        else {
            return;
        };
        let permalink = howto_permalink(&key);
        let observation = Observation {
            category: PATH_CATEGORY.to_string(),
            content: task.path.join(" → "),
            tags: vec![key.clone()],
            context: None,
        };
        let existing = self.memory.read(&permalink).await.ok().flatten();
        match existing {
            Some(existing) => {
                if existing.observations.iter().any(|item| {
                    item.category == observation.category && item.content == observation.content
                }) {
                    return;
                }
                if self
                    .memory
                    .append_observation(&permalink, observation)
                    .await
                    .is_err()
                {
                    return;
                }
            }
            None => {
                let title = format!("{key} 操作路径");
                let note = Note {
                    title: title.clone(),
                    permalink: permalink.clone(),
                    note_type: HOWTO_TYPE.to_string(),
                    tags: vec![HOWTO_TYPE.to_string()],
                    extra_frontmatter: Default::default(),
                    body: NoteText::body(&title, std::slice::from_ref(&observation), &[], ""),
                    observations: vec![observation],
                    relations: Vec::new(),
                    updated_at_millis: (self.clock)(),
                };
                if self.memory.write(note).await.is_err() {
                    return;
                }
            }
        }
        task.emit(AgentEvent::MemoryUpdated {
            task_id: task.task_id.clone(),
            permalink,
        });
    }
}

pub fn howto_permalink(key: &str) -> String {
    format!("howto/{key}")
}

#[async_trait]
impl Agent for XiaomanAgent {
    async fn handle(
        &self,
        user_message: &str,
        listener: Arc<dyn AgentListener>,
        stop: Arc<dyn StopSignal>,
    ) {
        let task_id = (self.id_factory)();
        let emit = {
            let listener = listener.clone();
            move |event: AgentEvent| listener.on_event(&event)
        };
        let tools = ChatTools {
            memory: self.memory.clone(),
            skills: self.skills.clone(),
            clock: self.clock.clone(),
            reminders: self.reminders.clone(),
            external: self.external_tools.clone(),
            phone_tasks: self.config.phone_tasks,
        };

        let built_in = self.built_in_specs();
        let external = self.external_specs().await;
        let has_external = !external.is_empty();
        let mut tool_specs = built_in.clone();
        tool_specs.extend(external);

        let system = ChatMessage::system(self.system_prompt(user_message, has_external).await);
        let past: Vec<ChatMessage> = self.history.lock().unwrap().iter().cloned().collect();
        let mut turn: Vec<ChatMessage> = vec![ChatMessage::user(user_message)];
        let mut last_text: Option<String> = None;
        let mut final_text: Option<String> = None;

        for round in 0..=MAX_TOOL_ROUNDS {
            let offer_tools = round < MAX_TOOL_ROUNDS;
            let mut messages = Vec::with_capacity(1 + past.len() + turn.len());
            messages.push(system.clone());
            messages.extend(past.iter().cloned());
            messages.extend(turn.iter().cloned());
            let response = match self
                .chat
                .complete(ChatRequest {
                    messages,
                    tools: if offer_tools {
                        tool_specs.clone()
                    } else {
                        Vec::new()
                    },
                    json_mode: false,
                })
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    emit(AgentEvent::Failed {
                        task_id,
                        message: prompts::llm_failed(&error),
                    });
                    return;
                }
            };
            let text = response.text.trim().to_string();
            if response.tool_calls.is_empty() || !offer_tools {
                final_text = Some(if text.is_empty() {
                    if response.tool_calls.is_empty() {
                        String::new()
                    } else {
                        prompts::TOOL_ROUNDS_EXCEEDED.to_string()
                    }
                } else {
                    text
                });
                break;
            }
            if !text.is_empty() {
                emit(AgentEvent::Reply {
                    task_id: task_id.clone(),
                    text: text.clone(),
                });
                last_text = Some(text);
            }
            turn.push(ChatMessage {
                role: ChatRole::Assistant,
                parts: vec![ContentPart::Text(response.text.clone())],
                tool_calls: response.tool_calls.clone(),
                tool_call_id: None,
            });
            for call in &response.tool_calls {
                let output = self
                    .run_tool(&tools, call, &task_id, &listener, &stop, &emit)
                    .await;
                turn.push(ChatMessage::tool(&call.id, output));
            }
            if stop.stopped() {
                break;
            }
        }

        if let Some(text) = final_text.clone().filter(|text| !text.is_empty()) {
            emit(AgentEvent::Reply {
                task_id: task_id.clone(),
                text,
            });
        }
        let remembered = final_text.filter(|text| !text.is_empty()).or(last_text);
        self.remember(user_message, remembered).await;
    }

    async fn run_phone_task(
        &self,
        goal: &str,
        skill_name: Option<&str>,
        listener: Arc<dyn AgentListener>,
        stop: Arc<dyn StopSignal>,
    ) -> AgentEvent {
        let task_id = (self.id_factory)();
        let finish = |success: bool, summary: String, steps: i32| -> AgentEvent {
            let event = AgentEvent::PhoneTaskFinished {
                task_id: task_id.clone(),
                success,
                summary,
                steps,
            };
            listener.on_event(&event);
            event
        };

        let wanted = skill_name.map(str::trim).filter(|name| !name.is_empty());
        let skill = match wanted {
            Some(name) => self.skills.get(name).await,
            None => None,
        };
        if let Some(skill) = &skill {
            if skill.channel == SkillChannel::Chat {
                return finish(
                    false,
                    format!("{}：{}", prompts::CHAT_SKILL_PHONE_REFUSED, skill.name),
                    0,
                );
            }
            listener.on_event(&AgentEvent::SkillChosen {
                task_id: task_id.clone(),
                skill_name: skill.name.clone(),
            });
        }
        listener.on_event(&AgentEvent::PhoneTaskStarted {
            task_id: task_id.clone(),
            goal: goal.to_string(),
            skill_name: skill.as_ref().map(|skill| skill.name.clone()),
        });
        if let (Some(wanted), None) = (wanted, &skill) {
            return finish(
                false,
                format!("{}：{}", prompts::SKILL_NOT_FOUND, wanted),
                0,
            );
        }

        let skill = skill.map(Arc::new);
        let mut task = TaskRun::new(task_id.clone(), goal, skill.clone(), listener.clone(), stop);
        task.references = self.references(skill.as_deref()).await;
        task.howto = self.howto(goal, skill.as_deref()).await;

        let flow = skill
            .as_ref()
            .and_then(|skill| skill.flow.clone())
            .filter(|flow| !flow.steps.is_empty());
        let outcome = match flow {
            Some(flow) => {
                crate::flow_step::run(&self.operator_loop, &self.users, &mut task, &flow).await
            }
            None => {
                self.operator_loop
                    .run(&mut task, None, None, self.config.max_steps)
                    .await
            }
        };

        if outcome.success && self.config.learn_from_success {
            self.learn(&task).await;
        }
        finish(outcome.success, outcome.summary, task.steps)
    }
}

impl XiaomanAgent {
    async fn run_tool(
        &self,
        tools: &ChatTools,
        call: &ToolCall,
        task_id: &str,
        listener: &Arc<dyn AgentListener>,
        stop: &Arc<dyn StopSignal>,
        emit: &(dyn Fn(AgentEvent) + Send + Sync),
    ) -> String {
        if call.name == agent_tools::PHONE_TASK && self.config.phone_tasks {
            let args = crate::chat_tools::parse_args(&call.arguments_json);
            let args = match args {
                Ok(args) => args,
                Err(_) => {
                    return "{\"error\":\"参数错误：arguments 不是合法的 JSON 对象\"}".to_string();
                }
            };
            let Some(goal) = JsonText::string(&args, "goal") else {
                return "{\"error\":\"参数错误：缺少 goal\"}".to_string();
            };
            let skill = JsonText::string(&args, "skill");
            let event = self
                .run_phone_task(&goal, skill.as_deref(), listener.clone(), stop.clone())
                .await;
            return phone_result_json(&event);
        }
        tools.execute(call, task_id, emit).await
    }
}
