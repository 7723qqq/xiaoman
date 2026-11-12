//! MCP services: the Android catalog with switches and keys, custom HTTP servers, save and test.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    div, prelude::*, rgb, Context, Entity, IntoElement, ParentElement, Render, Styled, Task, Window,
};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::InputState;
use gpui_kit::component::switch::Switch;
use xiaoman_app::session::Session;
use xiaoman_mcp::{
    CatalogServer, ConfirmPolicy, CustomMcpServer, McpCatalog, McpPlatform, McpSelection,
    ServerStatus,
};

use crate::ui::{field, hint, input, title, value};

/// Ids of 推荐组合: free or easy to obtain, read-only. Same two as the Kotlin app.
const RECOMMENDED: [&str; 2] = ["amap", "aihot"];

struct ServerRow {
    server: CatalogServer,
    enabled: bool,
    inputs: Vec<(String, Entity<InputState>)>,
}

pub struct ServicesView {
    session: Option<Arc<Session>>,
    rows: Vec<ServerRow>,
    custom: Vec<CustomMcpServer>,
    add_title: Entity<InputState>,
    add_url: Entity<InputState>,
    add_auth: Entity<InputState>,
    add_confirm: ConfirmPolicy,
    statuses: BTreeMap<String, ServerStatus>,
    message: Option<String>,
    pending: Arc<Mutex<Option<Vec<ServerStatus>>>>,
    poll: Option<Task<()>>,
}

impl ServicesView {
    pub fn new(session: Option<Arc<Session>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selection = session
            .as_ref()
            .map(|session| session.mcp_config())
            .unwrap_or_default();
        let catalog = McpCatalog::built_in();
        let rows = catalog
            .servers
            .iter()
            .filter(|server| server.available_on(McpPlatform::Android))
            .map(|server| {
                let inputs = server
                    .keys
                    .iter()
                    .map(|key| {
                        let initial = selection.keys.get(&key.name).cloned().unwrap_or_default();
                        (
                            key.name.clone(),
                            input(window, cx, key.label.clone(), initial),
                        )
                    })
                    .collect();
                ServerRow {
                    enabled: selection.enabled.contains(&server.id),
                    server: server.clone(),
                    inputs,
                }
            })
            .collect();
        let add_title = input(window, cx, "名称", "");
        let add_url = input(window, cx, "Streamable HTTP 地址", "");
        let add_auth = input(window, cx, "Authorization（可选）", "");
        Self {
            session,
            rows,
            custom: selection.custom.clone(),
            add_title,
            add_url,
            add_auth,
            add_confirm: ConfirmPolicy::Writes,
            statuses: BTreeMap::new(),
            message: None,
            pending: Arc::new(Mutex::new(None)),
            poll: None,
        }
    }

    fn selection(&self, cx: &Context<Self>) -> McpSelection {
        let mut enabled = BTreeSet::new();
        let mut keys = BTreeMap::new();
        for row in &self.rows {
            if row.enabled {
                enabled.insert(row.server.id.clone());
            }
            for (name, input) in &row.inputs {
                let value = value(input, cx);
                if !value.is_empty() {
                    keys.insert(name.clone(), value);
                }
            }
        }
        McpSelection {
            enabled,
            keys,
            custom: self.custom.clone(),
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            self.message = Some("会话还没有打开。".to_string());
            cx.notify();
            return;
        };
        let selection = self.selection(cx);
        self.message = Some(match session.save_mcp(selection) {
            Ok(()) => "已保存。".to_string(),
            Err(error) => format!("保存失败：{error}"),
        });
        cx.notify();
    }

    fn test(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            self.message = Some("会话还没有打开。".to_string());
            cx.notify();
            return;
        };
        let selection = self.selection(cx);
        self.testing_started(cx);
        let pending = self.pending.clone();
        std::thread::spawn(move || {
            let statuses = session.test_mcp(&selection);
            *pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(statuses);
        });
        let pending = self.pending.clone();
        self.poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let done = pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(statuses) = done {
                let _ = this.update(cx, |this, cx| {
                    this.statuses = statuses
                        .into_iter()
                        .map(|status| (status.name.clone(), status))
                        .collect();
                    this.message = Some("测试完成。".to_string());
                    cx.notify();
                });
                break;
            }
        }));
    }

    fn testing_started(&mut self, cx: &mut Context<Self>) {
        self.message = Some("正在连接…".to_string());
        self.statuses.clear();
        cx.notify();
    }
}

impl Render for ServicesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let url_text = value(&self.add_url, cx);
        // An empty field is not an error yet; the Compose form validates only what was typed.
        let url_problem = if url_text.trim().is_empty() {
            None
        } else {
            McpCatalog::validate_custom_url(&url_text)
        };
        // The Compose screen groups the catalog by category, in catalog order.
        let mut groups: Vec<(String, Vec<&ServerRow>)> = Vec::new();
        for row in &self.rows {
            match groups
                .iter_mut()
                .find(|(name, _)| name == &row.server.category)
            {
                Some((_, group)) => group.push(row),
                None => groups.push((row.server.category.clone(), vec![row])),
            }
        }
        let groups = groups
            .into_iter()
            .map(|(category, rows)| {
                let cards = rows
                    .into_iter()
                    .map(|row| {
                        let id = row.server.id.clone();
                        let checked = row.enabled;
                        let switch = Switch::new(format!("server-{id}"))
                            .checked(checked)
                            .on_change(cx.listener(move |this, checked, _, cx| {
                                if let Some(row) =
                                    this.rows.iter_mut().find(|row| row.server.id == id)
                                {
                                    row.enabled = *checked;
                                }
                                cx.notify();
                            }));
                        let status = self.statuses.get(&row.server.id).map(|status| {
                            if status.connected {
                                format!("已连接，{} 个工具", status.tool_count)
                            } else {
                                format!(
                                    "连接失败：{}",
                                    status
                                        .error
                                        .clone()
                                        .unwrap_or_else(|| "未知错误".to_string())
                                )
                            }
                        });
                        let missing = if row.enabled {
                            let labels: Vec<String> = row
                                .inputs
                                .iter()
                                .filter(|(_, input)| value(input, cx).trim().is_empty())
                                .map(|(name, _)| {
                                    row.server
                                        .keys
                                        .iter()
                                        .find(|key| &key.name == name)
                                        .map(|key| key.label.clone())
                                        .unwrap_or_else(|| name.clone())
                                })
                                .collect();
                            if labels.is_empty() {
                                None
                            } else {
                                Some(format!("还没有填写：{}", labels.join("、")))
                            }
                        } else {
                            None
                        };
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .rounded_lg()
                            .bg(rgb(0xffffff))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .text_base()
                                                            .child(row.server.title.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(rgb(0x1e6b52))
                                                            .child(if row.server.official {
                                                                "官方"
                                                            } else {
                                                                "社区"
                                                            }),
                                                    ),
                                            )
                                            .child(hint(format!(
                                                "{} · {}",
                                                if row.server.remote() {
                                                    "远程"
                                                } else {
                                                    "本地进程"
                                                },
                                                row.server.id
                                            ))),
                                    )
                                    .child(switch),
                            )
                            .child(div().text_sm().child(row.server.description.clone()))
                            .children(row.inputs.iter().map(|(name, input)| {
                                let help = row
                                    .server
                                    .keys
                                    .iter()
                                    .find(|key| &key.name == name)
                                    .and_then(|key| key.help.clone());
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(field(name, input))
                                    .when_some(help, |this, help| {
                                        this.child(
                                            Button::new(format!("apply-{}-{name}", row.server.id))
                                                .label("去申请")
                                                .on_click(
                                                    cx.listener(move |_, _, _, _| open_uri(&help)),
                                                ),
                                        )
                                    })
                            }))
                            .when_some(missing, |this, missing| {
                                this.child(div().text_sm().text_color(rgb(0xb3261e)).child(missing))
                            })
                            .when_some(status, |this, status| this.child(hint(status)))
                    })
                    .collect::<Vec<_>>();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_base().child(category))
                    .children(cards)
            })
            .collect::<Vec<_>>();

        let custom = self
            .custom
            .iter()
            .enumerate()
            .map(|(index, server)| {
                let status = self
                    .statuses
                    .get(&format!("custom-{}", index + 1))
                    .map(|status| {
                        if status.connected {
                            format!("已连接，{} 个工具", status.tool_count)
                        } else {
                            format!(
                                "连接失败：{}",
                                status
                                    .error
                                    .clone()
                                    .unwrap_or_else(|| "未知错误".to_string())
                            )
                        }
                    });
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(0xffffff))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_base().child(server.title.clone()))
                                    .child(hint(
                                        server
                                            .url
                                            .split('?')
                                            .next()
                                            .unwrap_or(server.url.as_str())
                                            .to_string(),
                                    ))
                                    .child(hint(format!(
                                        "{} · {}",
                                        confirm_label(server.confirm),
                                        headers_label(&server.headers)
                                    ))),
                            )
                            .child(
                                Button::new(format!("remove-custom-{index}"))
                                    .danger()
                                    .label("删除")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if index < this.custom.len() {
                                            this.custom.remove(index);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when_some(status, |this, status| this.child(hint(status)))
            })
            .collect::<Vec<_>>();

        div()
            .size_full()
            .id("services")
            .overflow_y_scroll()
            .child(
                div()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(title("外部服务")),
                    )
                    .child(hint("勾选后，小满可以直接调用这些服务查询或办事，不用操作手机。会修改数据或产生费用的操作，会先在聊天里问你。"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_sm().child(format!(
                                "已启用 {} 个",
                                self.custom.len() + self.rows.iter().filter(|row| row.enabled).count()
                            )))
                            .child(
                                Button::new("recommended")
                                    .label("推荐组合")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        for row in this.rows.iter_mut() {
                                            if RECOMMENDED.contains(&row.server.id.as_str()) {
                                                row.enabled = true;
                                            }
                                        }
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when_some(self.message.clone(), |this, message| {
                        this.child(div().text_sm().child(message))
                    })
                    .children(groups)
                    .child(title("其他 MCP 服务"))
                    .child(hint("只添加你信任的服务。地址需要是 https，支持 Streamable HTTP 的 MCP 服务都可以接入。"))
                    .children(custom)
                    .child(field("名称", &self.add_title))
                    .child(field("地址（https://…）", &self.add_url))
                    .child(field("Authorization 请求头（可选）", &self.add_auth))
                    .when_some(url_problem, |this, problem| {
                        this.child(div().text_sm().child(problem.to_string()))
                    })
                    .child(div().text_sm().child("调用前确认"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(confirm_chip(
                                self.add_confirm,
                                ConfirmPolicy::Writes,
                                "writes",
                                "会修改数据时确认",
                                cx,
                            ))
                            .child(confirm_chip(self.add_confirm, ConfirmPolicy::All, "all", "每次都确认", cx))
                            .child(confirm_chip(
                                self.add_confirm,
                                ConfirmPolicy::None,
                                "none",
                                "从不确认",
                                cx,
                            )),
                    )
                    .child(
                        Button::new("add-custom")
                            .label("添加")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let title = value(&this.add_title, cx);
                                let url = value(&this.add_url, cx);
                                let auth = value(&this.add_auth, cx);
                                if !title.is_empty() && McpCatalog::validate_custom_url(&url).is_none() {
                                    let headers = if auth.is_empty() {
                                        BTreeMap::new()
                                    } else {
                                        BTreeMap::from([("Authorization".to_string(), auth)])
                                    };
                                    this.custom.push(
                                        CustomMcpServer::new(title, url)
                                            .with_headers(headers)
                                            .with_confirm(this.add_confirm),
                                    );
                                    this.add_title
                                        .update(cx, |state, cx| state.set_value("", window, cx));
                                    this.add_url
                                        .update(cx, |state, cx| state.set_value("", window, cx));
                                    this.add_auth
                                        .update(cx, |state, cx| state.set_value("", window, cx));
                                    this.add_confirm = ConfirmPolicy::Writes;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("save-mcp")
                                    .primary()
                                    .label("保存")
                                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                            )
                            .child(
                                Button::new("test-mcp")
                                    .label("测试连接")
                                    .on_click(cx.listener(|this, _, _, cx| this.test(cx))),
                            ),
                    )
                    .child(hint("自定义服务只支持 Streamable HTTP；保存后从下一条消息开始生效。")),
            )
    }
}

/// One of the three 调用前确认 chips; `toggled` is the component library's selected state.
/// The catalog's 去申请 links open in the browser, like `LocalUriHandler.openUri` in Compose.
#[cfg(target_os = "android")]
fn open_uri(uri: &str) {
    xiaoman_android::session::open_uri(uri);
}

#[cfg(not(target_os = "android"))]
fn open_uri(_uri: &str) {}

fn confirm_chip(
    current: ConfirmPolicy,
    policy: ConfirmPolicy,
    id: &'static str,
    label: &'static str,
    cx: &mut Context<ServicesView>,
) -> Button {
    Button::new(format!("confirm-{id}"))
        .label(label)
        .toggled(current == policy)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.add_confirm = policy;
            cx.notify();
        }))
}

/// `mcp_confirm_writes` and friends: the policy in the words the Kotlin screen uses.
fn confirm_label(policy: ConfirmPolicy) -> &'static str {
    match policy {
        ConfirmPolicy::Writes => "会修改数据时确认",
        ConfirmPolicy::All => "每次都确认",
        ConfirmPolicy::None => "从不确认",
    }
}

/// Header names only: the values are secrets and must not be drawn.
fn headers_label(headers: &BTreeMap<String, String>) -> String {
    if headers.is_empty() {
        "无自定义请求头".to_string()
    } else {
        format!(
            "请求头：{}",
            headers.keys().cloned().collect::<Vec<_>>().join("、")
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use xiaoman_mcp::ConfirmPolicy;

    use super::{confirm_label, headers_label};

    #[test]
    fn confirm_labels_are_the_kotlin_words() {
        assert_eq!(confirm_label(ConfirmPolicy::Writes), "会修改数据时确认");
        assert_eq!(confirm_label(ConfirmPolicy::All), "每次都确认");
        assert_eq!(confirm_label(ConfirmPolicy::None), "从不确认");
    }

    #[test]
    fn header_labels_never_show_the_values() {
        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_string(), "Bearer secret".to_string());
        headers.insert("X-Api-Key".to_string(), "k-123".to_string());
        let label = headers_label(&headers);
        assert_eq!("请求头：Authorization、X-Api-Key", label);
        assert!(!label.contains("secret"), "{label}");
        assert!(!label.contains("k-123"), "{label}");
        assert_eq!("无自定义请求头", headers_label(&BTreeMap::new()));
    }
}
