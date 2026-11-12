//! The rule-based sentinel described in docs/ARCHITECTURE.md (Safety).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoman_api::{
    Action, ApprovalAnswer, ApprovalGateway, ApprovalRequest, ApprovalScope, AuditRecord,
    AuditSink, Decision, GuardContext, Point, ScreenState, Sentinel, Skill, UiNode,
};

mod confirm_words;

pub use confirm_words::BUILT_IN_CONFIRM_WORDS;
pub use confirm_words::matches as confirm_word_matches;
use confirm_words::{SchemeHost, longest_in, normalize};

/// Common home-screen packages: AOSP, Pixel, Samsung, Xiaomi, Huawei, Honor, OPPO/realme, vivo,
/// OnePlus, Meizu.
pub const DEFAULT_LAUNCHERS: &[&str] = &[
    "com.android.launcher",
    "com.android.launcher2",
    "com.android.launcher3",
    "com.google.android.apps.nexuslauncher",
    "com.sec.android.app.launcher",
    "com.miui.home",
    "com.huawei.android.launcher",
    "com.hihonor.android.launcher",
    "com.oppo.launcher",
    "com.bbk.launcher2",
    "net.oneplus.launcher",
    "com.meizu.flyme.launcher",
];

pub struct SentinelOptions {
    pub launcher_packages: HashSet<String>,
    pub self_package: String,
    pub extra_confirm_words: Vec<String>,
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl Default for SentinelOptions {
    fn default() -> Self {
        Self {
            launcher_packages: DEFAULT_LAUNCHERS.iter().map(|p| (*p).to_string()).collect(),
            self_package: "dev.xiaoman.app".to_string(),
            extra_confirm_words: Vec::new(),
            clock: Arc::new(xiaoman_api::now_millis),
        }
    }
}

/// Rules, first match wins:
///
/// 1. deny
///    - any device action on a secure (FLAG_SECURE) screen except `back`, `home` and `wait`;
///      `ask_user` and `finish` never touch the device and stay allowed;
///    - `type` into a password node (by `nodeId`, else the focused editable node);
///    - when the skill lists `apps`: `launch` / `deeplink` naming a package outside them, and
///      `tap`, `long_press`, `type` while the foreground package is outside them. Launchers and
///      `self_package` are always permitted.
/// 2. ask
///    - `tap`, `long_press`, `type` whose target, or the acted-on node's text or description,
///      contains a confirmation word: the built-in list, the skill's `confirm_before`, plus
///      `extra_confirm_words`. Neither source can remove a built-in word;
///    - `deeplink` whose URI matches none of the skill's `deeplinks` templates by scheme and host.
/// 3. allow everything else.
pub struct PolicySentinel {
    approvals: Arc<dyn ApprovalGateway>,
    audit: Arc<dyn AuditSink>,
    launcher_packages: HashSet<String>,
    self_package: String,
    base_words: Vec<String>,
    /// task id -> approval keys granted with [`ApprovalScope::Task`].
    task_approvals: Mutex<HashMap<String, HashSet<String>>>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

struct Verdict {
    decision: Decision,
    approval_key: Option<String>,
}

impl PolicySentinel {
    pub fn new(approvals: Arc<dyn ApprovalGateway>, audit: Arc<dyn AuditSink>) -> Self {
        Self::with_options(SentinelOptions::default(), approvals, audit)
    }

    pub fn with_options(
        options: SentinelOptions,
        approvals: Arc<dyn ApprovalGateway>,
        audit: Arc<dyn AuditSink>,
    ) -> Self {
        let mut words = BUILT_IN_CONFIRM_WORDS
            .iter()
            .map(|w| (*w).to_string())
            .collect::<Vec<_>>();
        words.extend(options.extra_confirm_words);
        Self {
            approvals,
            audit,
            launcher_packages: options.launcher_packages,
            self_package: options.self_package,
            base_words: normalize(&words),
            task_approvals: Mutex::new(HashMap::new()),
            clock: options.clock,
        }
    }

    /// Forgets the TASK-scoped approvals of `task_id`.
    pub fn end_task(&self, task_id: &str) {
        self.task_approvals.lock().unwrap().remove(task_id);
    }

    fn record(
        &self,
        context: &GuardContext,
        action: &Action,
        decision: &Decision,
        approved: Option<bool>,
    ) {
        self.audit.record(AuditRecord {
            time_millis: (self.clock)(),
            task_id: context.task_id.clone(),
            step_index: context.step_index,
            action: action.clone(),
            decision: decision.clone(),
            approved,
        });
    }

    fn evaluate(&self, context: &GuardContext, action: &Action) -> Verdict {
        if let Some(reason) = self.deny(context, action) {
            return Verdict {
                decision: Decision::Deny { reason },
                approval_key: None,
            };
        }
        self.ask(context, action).unwrap_or(Verdict {
            decision: Decision::allow(),
            approval_key: None,
        })
    }

    fn deny(&self, context: &GuardContext, action: &Action) -> Option<String> {
        let screen = context.screen.as_deref();
        if screen.is_some_and(|s| s.secure) && touches_device(action) && !is_navigation(action) {
            return Some("secure screen: the user must take over".to_string());
        }
        if let (Action::TypeText { .. }, Some(screen)) = (action, screen) {
            if type_target(screen, action).is_some_and(|node| node.password) {
                return Some("typing into a password field".to_string());
            }
        }
        let skill = context.skill.as_deref()?;
        if skill.apps.is_empty() {
            return None;
        }
        match action {
            Action::LaunchApp { package_name, .. } => {
                if !self.permitted(skill, package_name) {
                    return Some(format!(
                        "package {package_name} is not in the apps of skill {}",
                        skill.name
                    ));
                }
            }
            Action::OpenDeeplink {
                package_name: Some(package_name),
                ..
            } => {
                if !self.permitted(skill, package_name) {
                    return Some(format!(
                        "package {package_name} is not in the apps of skill {}",
                        skill.name
                    ));
                }
            }
            Action::Tap { .. } | Action::LongPress { .. } | Action::TypeText { .. } => {
                if let Some(package_name) = screen.and_then(|s| s.package_name.as_deref()) {
                    if !self.permitted(skill, package_name) {
                        return Some(format!(
                            "screen package {package_name} is outside the apps of skill {}",
                            skill.name
                        ));
                    }
                }
            }
            _ => {}
        }
        None
    }

    fn permitted(&self, skill: &Skill, package_name: &str) -> bool {
        skill.apps.iter().any(|app| app == package_name)
            || package_name == self.self_package
            || self.launcher_packages.contains(package_name)
    }

    fn ask(&self, context: &GuardContext, action: &Action) -> Option<Verdict> {
        match action {
            Action::Tap { .. } | Action::LongPress { .. } | Action::TypeText { .. } => {
                self.ask_for_word(context, action)
            }
            Action::OpenDeeplink { uri, .. } => {
                self.ask_for_deeplink(context.skill.as_deref(), uri, action)
            }
            _ => None,
        }
    }

    fn ask_for_word(&self, context: &GuardContext, action: &Action) -> Option<Verdict> {
        let words = self.words_for(context.skill.as_deref());
        let node = context
            .screen
            .as_deref()
            .and_then(|screen| node_for(screen, action));
        let mut candidates: Vec<(&str, &str)> = Vec::new();
        if let Some(target) = action.target() {
            candidates.push(("target", target));
        }
        if let Some(text) = node.and_then(|n| n.text.as_deref()) {
            candidates.push(("node text", text));
        }
        if let Some(description) = node.and_then(|n| n.description.as_deref()) {
            candidates.push(("node description", description));
        }
        let mut best: Option<(&str, &str, &str)> = None;
        for (place, text) in candidates {
            let Some(word) = longest_in(text, &words) else {
                continue;
            };
            let better = match best {
                None => true,
                Some((current, _, _)) => word.chars().count() > current.chars().count(),
            };
            if better {
                best = Some((word, place, text));
            }
        }
        let (word, place, text) = best?;
        let label = ellipsize(text.trim(), 24);
        let prompt = match action {
            Action::LongPress { .. } => format!("即将长按「{label}」，是否继续？"),
            Action::TypeText { submit, .. } => {
                if *submit {
                    format!("即将在「{label}」输入并提交，是否继续？")
                } else {
                    format!("即将在「{label}」输入内容，是否继续？")
                }
            }
            _ => format!("即将点击「{label}」，是否继续？"),
        };
        Some(Verdict {
            decision: Decision::Ask {
                prompt,
                reason: format!("confirmation word '{word}' in {place}"),
            },
            approval_key: Some(format!("word:{}", word.to_lowercase())),
        })
    }

    fn ask_for_deeplink(
        &self,
        skill: Option<&Skill>,
        uri: &str,
        _action: &Action,
    ) -> Option<Verdict> {
        let target = SchemeHost::of(uri);
        if let (Some(skill), Some(target)) = (skill, target.as_ref()) {
            if skill
                .deeplinks
                .iter()
                .any(|(_, template)| SchemeHost::of(template).as_ref() == Some(target))
            {
                return None;
            }
        }
        let reason = match skill {
            None => "deeplink without a skill".to_string(),
            Some(skill) => format!(
                "deeplink matches none of the deeplinks of skill {}",
                skill.name
            ),
        };
        let prompt = format!("即将打开链接「{}」，是否继续？", ellipsize(uri, 80));
        Some(Verdict {
            decision: Decision::Ask { prompt, reason },
            approval_key: Some(format!(
                "deeplink:{}",
                target
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| uri.to_string())
            )),
        })
    }

    fn words_for(&self, skill: Option<&Skill>) -> Vec<String> {
        match skill {
            Some(skill) if !skill.confirm_before.is_empty() => {
                let mut words = self.base_words.clone();
                words.extend(skill.confirm_before.iter().cloned());
                normalize(&words)
            }
            _ => self.base_words.clone(),
        }
    }
}

#[async_trait]
impl Sentinel for PolicySentinel {
    async fn guard(&self, context: &GuardContext, action: &Action) -> Decision {
        self.evaluate(context, action).decision
    }

    async fn authorize(&self, context: &GuardContext, action: &Action) -> bool {
        let verdict = self.evaluate(context, action);
        let (allowed, recorded) = match &verdict.decision {
            Decision::Allow { .. } => (true, None),
            Decision::Deny { .. } => (false, None),
            Decision::Ask { prompt, reason } => {
                let remembered = match &verdict.approval_key {
                    Some(key) => self
                        .task_approvals
                        .lock()
                        .unwrap()
                        .get(&context.task_id)
                        .is_some_and(|keys| keys.contains(key)),
                    None => false,
                };
                if remembered {
                    (true, Some(true))
                } else {
                    let answer = self
                        .approvals
                        .request(ApprovalRequest {
                            task_id: context.task_id.clone(),
                            action: action.clone(),
                            prompt: prompt.clone(),
                            reason: reason.clone(),
                        })
                        .await;
                    match answer {
                        Ok(answer) => {
                            remember_if_task_scoped(
                                &self.task_approvals,
                                context,
                                &verdict.approval_key,
                                answer,
                            );
                            (answer.approved, Some(answer.approved))
                        }
                        // The approval card could not be shown: fail closed.
                        Err(_) => (false, Some(false)),
                    }
                }
            }
        };
        self.record(context, action, &verdict.decision, recorded);
        allowed
    }
}

fn remember_if_task_scoped(
    task_approvals: &Mutex<HashMap<String, HashSet<String>>>,
    context: &GuardContext,
    key: &Option<String>,
    answer: ApprovalAnswer,
) {
    if answer.approved && answer.scope == ApprovalScope::Task {
        if let Some(key) = key {
            task_approvals
                .lock()
                .unwrap()
                .entry(context.task_id.clone())
                .or_default()
                .insert(key.clone());
        }
    }
}

fn touches_device(action: &Action) -> bool {
    !matches!(action, Action::AskUser { .. } | Action::Finish { .. })
}

fn is_navigation(action: &Action) -> bool {
    matches!(
        action,
        Action::Back { .. } | Action::Home { .. } | Action::Wait { .. }
    )
}

fn node_for<'a>(screen: &'a ScreenState, action: &Action) -> Option<&'a UiNode> {
    match action {
        Action::Tap { node_id, point, .. } | Action::LongPress { node_id, point, .. } => {
            by_id(screen, *node_id).or_else(|| point.and_then(|point| at_point(screen, point)))
        }
        Action::TypeText { .. } => type_target(screen, action),
        _ => None,
    }
}

fn type_target<'a>(screen: &'a ScreenState, action: &Action) -> Option<&'a UiNode> {
    match action {
        Action::TypeText {
            node_id: Some(id), ..
        } => by_id(screen, Some(*id)),
        Action::TypeText { .. } => screen
            .nodes
            .iter()
            .find(|node| node.focused && node.editable),
        _ => None,
    }
}

fn by_id(screen: &ScreenState, id: Option<i32>) -> Option<&UiNode> {
    id.and_then(|wanted| screen.nodes.iter().find(|node| node.id == wanted))
}

/// Smallest node whose bounds contain `point`; on equal area the later (usually deeper) node.
fn at_point(screen: &ScreenState, point: Point) -> Option<&UiNode> {
    let mut best: Option<&UiNode> = None;
    let mut best_area = i64::MAX;
    for node in &screen.nodes {
        let b = &node.bounds;
        if point.x() < b.left || point.x() > b.right || point.y() < b.top || point.y() > b.bottom {
            continue;
        }
        let area = i64::from(b.right - b.left) * i64::from(b.bottom - b.top);
        if area <= best_area {
            best = Some(node);
            best_area = area;
        }
    }
    best
}

fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut shortened: String = text.chars().take(max.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}
