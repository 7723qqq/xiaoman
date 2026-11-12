//! The operator loop: observe, prompt the operator model, parse, gate through Sentinel, perform,
//! detect stuck screens. Also hosts single-step and screen checks used by flows.

use std::sync::Arc;

use xiaoman_api::Decision;
use xiaoman_api::{
    Action, AgentConfig, AgentEvent, ChatMessage, ChatRequest, ChatResponse, ChatRole, ContentPart,
    Device, GuardContext, LlmClient, LlmError, ScreenState, Sentinel, UserChannel,
};

use crate::json_text::JsonText;
use crate::operator_reply_parser::{ParsedReply, parse};
use crate::prompts;
use crate::screen_text;
use crate::task_run::{Executed, LoopOutcome, StepConstraint, TaskRun};

pub const PARSE_FAILURE_LIMIT: i32 = 3;

pub struct OperatorLoop {
    operator: Arc<dyn LlmClient>,
    device: Arc<dyn Device>,
    sentinel: Arc<dyn Sentinel>,
    users: Arc<dyn UserChannel>,
    config: AgentConfig,
}

impl OperatorLoop {
    pub fn new(
        operator: Arc<dyn LlmClient>,
        device: Arc<dyn Device>,
        sentinel: Arc<dyn Sentinel>,
        users: Arc<dyn UserChannel>,
        config: AgentConfig,
    ) -> Self {
        Self {
            operator,
            device,
            sentinel,
            users,
            config,
        }
    }

    pub fn max_steps(&self) -> i32 {
        self.config.max_steps
    }

    /// Runs until finish, failure or `max_steps` iterations. `sub_goal` replaces the task goal as the
    /// immediate objective (flow `ai` steps); `constraint` makes it a single constrained step.
    pub async fn run(
        &self,
        task: &mut TaskRun,
        sub_goal: Option<&str>,
        constraint: Option<&StepConstraint>,
        max_steps: i32,
    ) -> LoopOutcome {
        let mut history: Vec<String> = Vec::new();
        let mut feedback: Vec<String> = Vec::new();
        let mut parse_failures: i32 = 0;
        let mut acted_on: Option<String> = None;
        let mut acted = false;
        let mut same_after_acting = 0;
        let mut warned_stuck = false;

        for _ in 0..max_steps {
            if task.stop.stopped() {
                return LoopOutcome {
                    success: false,
                    summary: prompts::STOPPED.to_string(),
                };
            }
            let screen = match self.device.observe().await {
                Ok(screen) => Arc::new(screen),
                Err(error) => {
                    task.emit(AgentEvent::Failed {
                        task_id: task.task_id.clone(),
                        message: error.0.clone(),
                    });
                    return LoopOutcome {
                        success: false,
                        summary: error.0,
                    };
                }
            };
            if let Some(package) = &screen.package_name {
                task.last_package = Some(package.clone());
            }

            if screen.secure {
                task.emit(AgentEvent::NeedsUser {
                    task_id: task.task_id.clone(),
                    question: prompts::SECURE_SCREEN.to_string(),
                    takeover: true,
                });
                let answer = self
                    .users
                    .ask(&task.task_id, prompts::SECURE_SCREEN, true)
                    .await;
                history.push(prompts::takeover_done(&answer));
                acted = false;
                continue;
            }

            let fingerprint = screen.fingerprint();
            if acted {
                same_after_acting = if Some(&fingerprint) == acted_on.as_ref() {
                    same_after_acting + 1
                } else {
                    0
                };
                acted = false;
            }
            if same_after_acting > 0 && same_after_acting + 1 >= self.config.stuck_repeats {
                if warned_stuck {
                    return LoopOutcome {
                        success: false,
                        summary: prompts::STUCK_FAILED.to_string(),
                    };
                }
                warned_stuck = true;
                same_after_acting = 0;
                feedback.push(prompts::STUCK_WARNING.to_string());
            }

            let request =
                self.operator_request(task, sub_goal, constraint, &screen, &history, &feedback);
            let reply = match self.complete_with_retry(request).await {
                Ok(reply) => reply,
                Err(error) => {
                    let message = prompts::operator_llm_failed(&error);
                    task.emit(AgentEvent::Failed {
                        task_id: task.task_id.clone(),
                        message: message.clone(),
                    });
                    return LoopOutcome {
                        success: false,
                        summary: message,
                    };
                }
            };
            feedback.clear();

            let mut action = match parse(&reply.text) {
                ParsedReply::Error { message } => {
                    parse_failures += 1;
                    if parse_failures >= PARSE_FAILURE_LIMIT {
                        return LoopOutcome {
                            success: false,
                            summary: prompts::PARSE_FAILED.to_string(),
                        };
                    }
                    feedback.push(prompts::parse_error(&message));
                    continue;
                }
                ParsedReply::Ok { action, .. } => {
                    parse_failures = 0;
                    action
                }
            };

            if let Some(constraint) = constraint {
                if !matches!(action, Action::Finish { .. }) {
                    let matches_kind = match constraint.kind.as_str() {
                        "tap" => matches!(action, Action::Tap { .. }),
                        "type" => matches!(action, Action::TypeText { .. }),
                        _ => true,
                    };
                    if !matches_kind {
                        feedback.push(prompts::wrong_single_step_kind(&constraint.kind));
                        continue;
                    }
                    if let (
                        Action::TypeText {
                            node_id,
                            submit,
                            target,
                            reason,
                            ..
                        },
                        Some(text),
                    ) = (&action, &constraint.text)
                    {
                        action = Action::TypeText {
                            text: text.clone(),
                            node_id: *node_id,
                            submit: *submit,
                            target: target.clone(),
                            reason: reason.clone(),
                        };
                    }
                }
            }

            let node_id = match &action {
                Action::Tap { node_id, .. }
                | Action::LongPress { node_id, .. }
                | Action::TypeText { node_id, .. } => *node_id,
                _ => None,
            };
            if let Some(node_id) = node_id {
                if !screen.nodes.iter().any(|node| node.id == node_id) {
                    feedback.push(prompts::unknown_node(node_id));
                    continue;
                }
            }

            task.emit(AgentEvent::StepPlanned {
                task_id: task.task_id.clone(),
                step_index: task.steps,
                action: action.clone(),
            });
            match &action {
                Action::Finish {
                    success, summary, ..
                } => {
                    return LoopOutcome {
                        success: *success,
                        summary: summary.clone(),
                    };
                }
                Action::AskUser {
                    question, takeover, ..
                } => {
                    task.emit(AgentEvent::NeedsUser {
                        task_id: task.task_id.clone(),
                        question: question.clone(),
                        takeover: *takeover,
                    });
                    let answer = self.users.ask(&task.task_id, question, *takeover).await;
                    history.push(prompts::asked_user(question, &answer));
                    continue;
                }
                _ => {}
            }

            let index = task.steps;
            match self.execute(task, &action, Some(screen.clone())).await {
                Executed::Blocked { decision } => {
                    feedback.push(prompts::blocked(&decision));
                    history.push(prompts::step_line(
                        index,
                        &screen_text::describe(&action),
                        &format!("被拦截：{}", decision.reason()),
                    ));
                }
                Executed::Done { result } => {
                    let outcome = if result.ok {
                        "成功".to_string()
                    } else {
                        format!(
                            "失败：{}",
                            result
                                .message
                                .clone()
                                .unwrap_or_else(|| "未知原因".to_string())
                        )
                    };
                    history.push(prompts::step_line(
                        index,
                        &screen_text::describe(&action),
                        &outcome,
                    ));
                    if !result.ok {
                        feedback.push(format!(
                            "上一步执行失败：{}",
                            result
                                .message
                                .clone()
                                .unwrap_or_else(|| "未知原因".to_string())
                        ));
                    }
                    acted = true;
                    acted_on = Some(fingerprint);
                    if constraint.is_some() {
                        let summary = if result.ok {
                            screen_text::describe(&action)
                        } else {
                            result
                                .message
                                .clone()
                                .unwrap_or_else(|| "执行失败".to_string())
                        };
                        return LoopOutcome {
                            success: result.ok,
                            summary,
                        };
                    }
                }
            }
        }

        LoopOutcome {
            success: false,
            summary: match constraint {
                Some(constraint) => format!("没能完成：{}", constraint.target),
                None => prompts::MAX_STEPS.to_string(),
            },
        }
    }

    /// Gates `action` through Sentinel and performs it; emits StepBlocked or StepExecuted.
    pub async fn execute(
        &self,
        task: &mut TaskRun,
        action: &Action,
        screen: Option<Arc<ScreenState>>,
    ) -> Executed {
        let context = GuardContext {
            task_id: task.task_id.clone(),
            goal: task.goal.clone(),
            skill: task.skill.clone(),
            screen: screen.clone(),
            step_index: task.steps,
        };
        let needs_confirmation = matches!(
            self.sentinel.guard(&context, action).await,
            Decision::Ask { .. }
        );
        if !self.sentinel.authorize(&context, action).await {
            let decision = match self.sentinel.guard(&context, action).await {
                Decision::Allow { .. } => Decision::Deny {
                    reason: "未获授权".to_string(),
                },
                other => other,
            };
            task.emit(AgentEvent::StepBlocked {
                task_id: task.task_id.clone(),
                step_index: task.steps,
                action: action.clone(),
                decision: decision.clone(),
            });
            return Executed::Blocked { decision };
        }
        if task.stop.stopped() {
            let decision = Decision::Deny {
                reason: prompts::STOPPED.to_string(),
            };
            task.emit(AgentEvent::StepBlocked {
                task_id: task.task_id.clone(),
                step_index: task.steps,
                action: action.clone(),
                decision: decision.clone(),
            });
            return Executed::Blocked { decision };
        }
        if needs_confirmation && !self.await_approval_screen(task, screen.as_deref()).await {
            let decision = Decision::Deny {
                reason:
                    "确认期间画面发生变化，未执行原动作；请返回目标应用，重新观察和定位后再确认。"
                        .to_string(),
            };
            task.emit(AgentEvent::StepBlocked {
                task_id: task.task_id.clone(),
                step_index: task.steps,
                action: action.clone(),
                decision: decision.clone(),
            });
            return Executed::Blocked { decision };
        }
        if task.stop.stopped() {
            let decision = Decision::Deny {
                reason: prompts::STOPPED.to_string(),
            };
            task.emit(AgentEvent::StepBlocked {
                task_id: task.task_id.clone(),
                step_index: task.steps,
                action: action.clone(),
                decision: decision.clone(),
            });
            return Executed::Blocked { decision };
        }

        let result = self.device.perform(action.clone()).await;
        task.emit(AgentEvent::StepExecuted {
            task_id: task.task_id.clone(),
            step_index: task.steps,
            action: action.clone(),
            result: result.clone(),
        });
        task.steps += 1;
        if result.ok {
            if let Action::LaunchApp { package_name, .. } = action {
                if task.launched_package.is_none() {
                    task.launched_package = Some(package_name.clone());
                }
            }
            let label = screen_text::path_label(action, screen.as_deref());
            task.add_path(label.as_deref());
        }
        Executed::Done { result }
    }

    /// App-switch animations can outlast the approval card. Never tap until the original UI returns.
    async fn await_approval_screen(&self, task: &TaskRun, before: Option<&ScreenState>) -> bool {
        for attempt in 0..5 {
            if task.stop.stopped() {
                return false;
            }
            let Ok(after) = self.device.observe().await else {
                return false;
            };
            if after.secure {
                return false;
            }
            if screen_text::matches_approval_screen(before, &after) {
                return true;
            }
            if attempt < 4 {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        }
        false
    }

    /// Flow `assert`: asks the operator model whether the screen shows `expectation`.
    pub async fn check(&self, task: &mut TaskRun, expectation: &str) -> LoopOutcome {
        for _ in 0..PARSE_FAILURE_LIMIT {
            if task.stop.stopped() {
                return LoopOutcome {
                    success: false,
                    summary: prompts::STOPPED.to_string(),
                };
            }
            let screen = match self.device.observe().await {
                Ok(screen) => screen,
                Err(error) => {
                    task.emit(AgentEvent::Failed {
                        task_id: task.task_id.clone(),
                        message: error.0.clone(),
                    });
                    return LoopOutcome {
                        success: false,
                        summary: error.0,
                    };
                }
            };
            if screen.secure {
                task.emit(AgentEvent::NeedsUser {
                    task_id: task.task_id.clone(),
                    question: prompts::SECURE_SCREEN.to_string(),
                    takeover: true,
                });
                self.users
                    .ask(&task.task_id, prompts::SECURE_SCREEN, true)
                    .await;
                continue;
            }
            let text = format!(
                "{}\n\n{}",
                prompts::assert_request(expectation),
                screen_text::section(&screen, self.config.max_nodes as usize)
            );
            let request = ChatRequest {
                messages: vec![
                    ChatMessage::system(prompts::ASSERT_SYSTEM),
                    user_message(text, &screen),
                ],
                tools: Vec::new(),
                json_mode: true,
            };
            let reply = match self.complete_with_retry(request).await {
                Ok(reply) => reply,
                Err(error) => {
                    let message = prompts::operator_llm_failed(&error);
                    task.emit(AgentEvent::Failed {
                        task_id: task.task_id.clone(),
                        message: message.clone(),
                    });
                    return LoopOutcome {
                        success: false,
                        summary: message,
                    };
                }
            };
            let Some(object) = JsonText::first_object(&reply.text) else {
                continue;
            };
            let Some(ok) = JsonText::bool_of(&object, "ok") else {
                continue;
            };
            let reason = JsonText::string(&object, "reason");
            let summary = if ok {
                reason.clone().unwrap_or_else(|| expectation.to_string())
            } else {
                prompts::assert_failed(expectation, reason.as_deref())
            };
            return LoopOutcome {
                success: ok,
                summary,
            };
        }
        LoopOutcome {
            success: false,
            summary: prompts::assert_failed(expectation, None),
        }
    }

    /// Retry a transient model failure once; account/configuration failures need intervention.
    pub async fn complete_with_retry(
        &self,
        request: ChatRequest,
    ) -> Result<ChatResponse, LlmError> {
        match self.operator.complete(request.clone()).await {
            Ok(response) => Ok(response),
            Err(error) if error.retryable => self.operator.complete(request).await,
            Err(error) => Err(error),
        }
    }

    fn operator_request(
        &self,
        task: &TaskRun,
        sub_goal: Option<&str>,
        constraint: Option<&StepConstraint>,
        screen: &ScreenState,
        history: &[String],
        feedback: &[String],
    ) -> ChatRequest {
        let mut sections: Vec<String> = Vec::new();
        sections.push(match sub_goal {
            Some(goal) => prompts::sub_goal_section(goal, &task.goal),
            None => prompts::goal_section(&task.goal),
        });
        if let Some(skill) = &task.skill {
            if !skill.instructions.trim().is_empty() {
                sections.push(prompts::skill_section(&skill.name, &skill.instructions));
            }
        }
        if !task.references.trim().is_empty() {
            sections.push(format!(
                "{}\n{}",
                prompts::MEMORY_REFS_HEADER,
                task.references
            ));
        }
        if !task.howto.trim().is_empty() {
            sections.push(format!("{}\n{}", prompts::HOWTO_HEADER, task.howto));
        }
        if !history.is_empty() {
            let window = self.config.history_window.max(0) as usize;
            let start = history.len().saturating_sub(window);
            sections.push(format!(
                "{}\n{}",
                prompts::HISTORY_HEADER,
                history[start..].join("\n")
            ));
        }
        if !feedback.is_empty() {
            sections.push(format!(
                "{}\n{}",
                prompts::FEEDBACK_HEADER,
                feedback.join("\n")
            ));
        }
        if let Some(constraint) = constraint {
            sections.push(prompts::single_step(
                &constraint.kind,
                &constraint.target,
                constraint.text.as_deref(),
            ));
        }
        sections.push(screen_text::section(screen, self.config.max_nodes as usize));
        sections.push(prompts::NEXT_STEP.to_string());

        ChatRequest {
            messages: vec![
                ChatMessage::system(prompts::operator_system(&self.config.operator_notes)),
                user_message(sections.join("\n\n"), screen),
            ],
            tools: Vec::new(),
            json_mode: true,
        }
    }
}

fn user_message(text: String, screen: &ScreenState) -> ChatMessage {
    let mut parts = vec![ContentPart::Text(text)];
    if let Some(png) = &screen.screenshot_png {
        parts.push(ContentPart::ImagePng(png.clone()));
    }
    ChatMessage::new(ChatRole::User, parts)
}
