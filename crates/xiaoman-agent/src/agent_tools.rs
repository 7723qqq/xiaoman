//! Tool definitions offered to the chat model.

use serde_json::{Map, Value, json};
use xiaoman_api::ToolSpec;

pub const MEMORY_SEARCH: &str = "memory_search";
pub const MEMORY_READ: &str = "memory_read";
pub const MEMORY_WRITE: &str = "memory_write";
pub const MEMORY_FORGET: &str = "memory_forget";
pub const LIST_SKILLS: &str = "list_skills";
pub const REMINDER_SET: &str = "reminder_set";
pub const PHONE_TASK: &str = "phone_task";

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: MEMORY_SEARCH.into(),
            description: "在用户的记忆笔记中全文搜索（支持中文）。".into(),
            parameters: schema(
                &["query"],
                &[
                    ("query", str_schema("搜索词")),
                    ("limit", int_schema("最多返回条数，默认 5")),
                ],
            ),
        },
        ToolSpec {
            name: MEMORY_READ.into(),
            description: "读取一条记忆笔记的全文。".into(),
            parameters: schema(
                &["identifier"],
                &[(
                    "identifier",
                    str_schema("笔记的 permalink（如 places/家）或标题"),
                )],
            ),
        },
        ToolSpec {
            name: MEMORY_WRITE.into(),
            description:
                "新建或更新一条记忆笔记；已有同名笔记时追加观察和关系（相同内容不会重复）。".into(),
            parameters: schema(
                &["title", "observations"],
                &[
                    ("title", str_schema("笔记标题，如 家")),
                    (
                        "permalink",
                        str_schema("可选，路径形式的唯一键，如 places/家；默认用标题"),
                    ),
                    (
                        "type",
                        str_schema("可选，笔记类型，如 place、person、preference；默认 note"),
                    ),
                    (
                        "observations",
                        array_schema(
                            "观察条目",
                            schema(
                                &["category", "content"],
                                &[
                                    ("category", str_schema("类别，如 address、preference")),
                                    ("content", str_schema("内容")),
                                    ("tags", array_schema("标签，不带 #", str_schema("标签"))),
                                    ("context", str_schema("可选，来源或补充说明")),
                                ],
                            ),
                        ),
                    ),
                    (
                        "relations",
                        array_schema(
                            "可选，与其他笔记的关系",
                            schema(
                                &["type", "target"],
                                &[
                                    ("type", str_schema("关系类型，如 lives_in")),
                                    ("target", str_schema("目标笔记标题")),
                                ],
                            ),
                        ),
                    ),
                ],
            ),
        },
        ToolSpec {
            name: MEMORY_FORGET.into(),
            description: "删除一条记忆笔记（真正删除文件）。仅在用户明确要求忘记时使用。".into(),
            parameters: schema(
                &["identifier"],
                &[("identifier", str_schema("笔记的 permalink 或标题"))],
            ),
        },
        ToolSpec {
            name: LIST_SKILLS.into(),
            description: "列出所有可用的手机操作技能。".into(),
            parameters: schema(&[], &[]),
        },
        ToolSpec {
            name: REMINDER_SET.into(),
            description: "设置一条定时提醒：到点后小满会在手机上弹出通知。".into(),
            parameters: schema(
                &["text", "at"],
                &[
                    ("text", str_schema("提醒内容，如 喝水、带伞")),
                    (
                        "at",
                        str_schema(
                            "触发时间，ISO-8601 本地时间（设备时区，不带时区后缀），如 2026-11-03T08:00",
                        ),
                    ),
                ],
            ),
        },
        ToolSpec {
            name: PHONE_TASK.into(),
            description: "在用户的手机上完成一个操作任务，返回结果摘要。".into(),
            parameters: schema(
                &["goal"],
                &[
                    (
                        "goal",
                        str_schema(
                            "具体目标，包含关键参数，如 用高德打车回家（北京市海淀区某某路 1 号），选快车",
                        ),
                    ),
                    ("skill", str_schema("可选，使用的技能名")),
                ],
            ),
        },
    ]
}

fn schema(required: &[&str], properties: &[(&str, Value)]) -> Value {
    let mut props = Map::new();
    for (key, value) in properties {
        props.insert((*key).to_string(), value.clone());
    }
    json!({
        "type": "object",
        "properties": Value::Object(props),
        "required": required,
    })
}

fn str_schema(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn int_schema(description: &str) -> Value {
    json!({"type": "integer", "description": description})
}

fn array_schema(description: &str, items: Value) -> Value {
    json!({"type": "array", "description": description, "items": items})
}
