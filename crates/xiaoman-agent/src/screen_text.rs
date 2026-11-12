//! Text views of screens and actions for prompts, history, learning notes and developer tools.

use xiaoman_api::{Action, ScreenState, UiNode};

use crate::prompts;

/// Approval is valid only for the same visible controls, prices and geometry.
pub fn matches_approval_screen(before: Option<&ScreenState>, after: &ScreenState) -> bool {
    let Some(before) = before else { return false };
    !before.secure
        && !after.secure
        && before.package_name.is_some()
        && before.package_name == after.package_name
        && before.activity_name == after.activity_name
        && before.width_px == after.width_px
        && before.height_px == after.height_px
        && !before.nodes.is_empty()
        && before.nodes == after.nodes
}

pub fn section(screen: &ScreenState, max_nodes: usize) -> String {
    let shown: Vec<&UiNode> = screen.nodes.iter().take(max_nodes).collect();
    let lines = shown
        .iter()
        .map(|node| node_line(node))
        .collect::<Vec<_>>()
        .join("\n");
    let truncated = screen.nodes.len().saturating_sub(shown.len());
    prompts::screen_section(
        screen.package_name.as_deref(),
        screen.activity_name.as_deref(),
        &lines,
        truncated,
    )
}

/// `[id] class "text" desc=... (l,t,r,b) clickable editable ...`
pub fn node_line(node: &UiNode) -> String {
    let class = node
        .class_name
        .as_deref()
        .map(|name| name.rsplit('.').next().unwrap_or(name))
        .unwrap_or("View");
    let mut out = format!("[{}] {class}", node.id);
    if let Some(text) = clean(node.text.as_deref()) {
        out.push_str(&format!(" \"{text}\""));
    }
    if let Some(description) = clean(node.description.as_deref()) {
        out.push_str(&format!(" desc={description}"));
    }
    if let Some(resource) = node
        .resource_id
        .as_deref()
        .and_then(|id| id.split(":id/").nth(1))
        .filter(|id| !id.trim().is_empty())
    {
        out.push_str(&format!(" res={resource}"));
    }
    let bounds = &node.bounds;
    out.push_str(&format!(
        " ({},{},{},{})",
        bounds.left, bounds.top, bounds.right, bounds.bottom
    ));
    if node.clickable {
        out.push_str(" clickable");
    }
    if node.editable {
        out.push_str(" editable");
    }
    if node.scrollable {
        out.push_str(" scrollable");
    }
    if node.focused {
        out.push_str(" focused");
    }
    if node.password {
        out.push_str(" password");
    }
    out
}

pub fn describe(action: &Action) -> String {
    match action {
        Action::Tap {
            point,
            node_id,
            target,
            ..
        } => {
            format!(
                "点击 {}",
                target
                    .clone()
                    .or_else(|| node_id.map(|id| format!("节点 {id}")))
                    .unwrap_or_else(|| point_text(*point))
            )
        }
        Action::LongPress {
            point,
            node_id,
            target,
            ..
        } => {
            format!(
                "长按 {}",
                target
                    .clone()
                    .or_else(|| node_id.map(|id| format!("节点 {id}")))
                    .unwrap_or_else(|| point_text(*point))
            )
        }
        Action::Swipe { from, to, .. } => {
            format!("滑动 ({},{})→({},{})", from.x(), from.y(), to.x(), to.y())
        }
        Action::TypeText { text, target, .. } => {
            let mut out = format!("输入「{text}」");
            if let Some(target) = target {
                out.push_str(&format!(" 到 {target}"));
            }
            out
        }
        Action::Back { .. } => "返回".to_string(),
        Action::Home { .. } => "回到桌面".to_string(),
        Action::LaunchApp { package_name, .. } => format!("打开 {package_name}"),
        Action::OpenDeeplink { uri, .. } => format!("打开链接 {uri}"),
        Action::Wait { millis, .. } => format!("等待 {millis}ms"),
        Action::AskUser { question, .. } => format!("询问用户：{question}"),
        Action::Finish { summary, .. } => format!("结束：{summary}"),
    }
}

/// Short label of a successful action for the learned path, or `None` for actions not worth recording.
pub fn path_label(action: &Action, screen: Option<&ScreenState>) -> Option<String> {
    let node_text = |id: Option<i32>| -> Option<String> {
        let id = id?;
        let node = screen?.nodes.iter().find(|node| node.id == id)?;
        clean(node.text.as_deref()).or_else(|| clean(node.description.as_deref()))
    };
    let target = action_target(action).and_then(|target| clean(Some(target)));
    match action {
        Action::Tap { node_id, .. } => target.or_else(|| node_text(*node_id)),
        Action::LongPress { node_id, .. } => Some(format!(
            "长按 {}",
            target.or_else(|| node_text(*node_id)).unwrap_or_default()
        )),
        Action::TypeText { node_id, .. } => Some(format!(
            "输入{}",
            target.or_else(|| node_text(*node_id)).unwrap_or_default()
        )),
        Action::LaunchApp { package_name, .. } => {
            Some(target.unwrap_or_else(|| format!("打开 {package_name}")))
        }
        Action::OpenDeeplink { .. } => Some(target.unwrap_or_else(|| "打开链接".to_string())),
        Action::Swipe { .. } => Some(target.unwrap_or_else(|| "滑动".to_string())),
        Action::Back { .. } => Some("返回".to_string()),
        Action::Home { .. }
        | Action::Wait { .. }
        | Action::AskUser { .. }
        | Action::Finish { .. } => None,
    }
}

fn action_target(action: &Action) -> Option<&str> {
    action.target()
}

fn point_text(point: Option<xiaoman_api::Point>) -> String {
    match point {
        Some(point) => format!("Point(x={}, y={})", point.x(), point.y()),
        None => "None".to_string(),
    }
}

fn clean(text: Option<&str>) -> Option<String> {
    let text = text?;
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.chars().count() > 60 {
        let mut clipped: String = collapsed.chars().take(60).collect();
        clipped.push('…');
        Some(clipped)
    } else {
        Some(collapsed)
    }
}
