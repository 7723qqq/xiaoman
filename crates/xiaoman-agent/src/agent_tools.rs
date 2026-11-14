//! Tool definitions offered to the chat model.

use serde_json::{Map, Value, json};
use xiaoman_api::ToolSpec;

pub const MEMORY_SEARCH: &str = "memory_search";
pub const MEMORY_READ: &str = "memory_read";
pub const MEMORY_WRITE: &str = "memory_write";
pub const MEMORY_FORGET: &str = "memory_forget";
pub const LIST_SKILLS: &str = "list_skills";
pub const SKILL_READ: &str = "skill_read";
pub const SKILL_EDIT: &str = "skill_edit";
pub const REMINDER_SET: &str = "reminder_set";
pub const PHONE_TASK: &str = "phone_task";
pub const VOCAB_TODAY: &str = "vocab_today";
pub const VOCAB_GRADE: &str = "vocab_grade";
pub const VOCAB_PREFS: &str = "vocab_prefs";
pub const VOCAB_ADD_WORD: &str = "vocab_add_word";
pub const VOCAB_STATUS: &str = "vocab_status";

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
            name: SKILL_READ.into(),
            description: "读取一个技能的 SKILL.md 全文；修改或删除前先用它看当前内容。".into(),
            parameters: schema(&["name"], &[("name", str_schema("技能名，如 taxi-home"))]),
        },
        ToolSpec {
            name: SKILL_EDIT.into(),
            description: "提议新建、修改或删除一个技能。调用后会弹出审批卡，只有用户点“保存”才写入；取消则不写盘。技能名必须匹配 [a-z0-9-]+；channel 为 chat 时不能设置 apps、confirm_before、deeplinks；正文不能包含密码、Key、手机号。create/update 需要 description、channel、instructions。".into(),
            parameters: schema(
                &["action", "name"],
                &[
                    (
                        "action",
                        enum_schema(&["create", "update", "delete"], "要执行的动作"),
                    ),
                    ("name", str_schema("技能名，如 daily-review")),
                    ("description", str_schema("一句话说明，单行，最长 200 字")),
                    (
                        "channel",
                        enum_schema(
                            &[
                                "api",
                                "cli",
                                "deeplink",
                                "gui",
                                "deeplink-then-gui",
                                "mixed",
                                "chat",
                            ],
                            "技能类型；纯聊天流程用 chat",
                        ),
                    ),
                    (
                        "instructions",
                        str_schema("SKILL.md 正文：给聊天模型或操作模型的 Markdown 指令"),
                    ),
                    (
                        "apps",
                        array_schema("允许操作的包名（仅非 chat）", str_schema("包名")),
                    ),
                    (
                        "confirm_before",
                        array_schema("点击/输入前需要确认的词（仅非 chat）", str_schema("词")),
                    ),
                    (
                        "memory",
                        array_schema("要加载进提示词的记忆笔记（如 places/家）", str_schema("permalink")),
                    ),
                    ("deeplinks", object_schema("命名深链（仅非 chat）：名字 -> URI")),
                ],
            ),
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

/// The vocabulary tools, only offered when a session has a vocabulary engine.
pub fn vocab_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: VOCAB_TODAY.into(),
            description: "取今天的背单词计划：到期复习词和今天的新词。第一次调用会生成今天的批次；日期、复习轮次和统计都以返回值为准，不要自己计算。".into(),
            parameters: schema(&[], &[]),
        },
        ToolSpec {
            name: VOCAB_GRADE.into(),
            description: "提交复习结果，引擎按艾宾浩斯曲线安排下次复习。只提交 vocab_today 返回的 due_words；correct 表示用户是否答对。".into(),
            parameters: schema(
                &["answers"],
                &[(
                    "answers",
                    array_schema(
                        "每个复习词的结果",
                        schema(
                            &["word", "correct"],
                            &[
                                ("word", str_schema("单词")),
                                ("correct", bool_schema("用户是否答对")),
                            ],
                        ),
                    ),
                )],
            ),
        },
        ToolSpec {
            name: VOCAB_STATUS.into(),
            description: "读取背单词进度和偏好，不会生成今天的批次。用户只问「背了多少」「还剩多少」时用它。".into(),
            parameters: schema(&[], &[]),
        },
        ToolSpec {
            name: VOCAB_PREFS.into(),
            description: "读取或修改背单词偏好：每天新词数、每日提醒时间、提醒开关。不传参数就是读取；修改后引擎会重新安排提醒。".into(),
            parameters: schema(
                &[],
                &[
                    ("daily_new", int_schema("每天新词数，0-100")),
                    ("reminder_time", str_schema("每日提醒时间，HH:MM，如 20:00")),
                    ("reminder_enabled", bool_schema("是否开启每日提醒")),
                ],
            ),
        },
        ToolSpec {
            name: VOCAB_ADD_WORD.into(),
            description: "把词库里没有的单词加入背词列表，至少给 word 和中文 meaning；加入后会出现在今天或明天的计划里。".into(),
            parameters: schema(
                &["word", "meaning"],
                &[
                    ("word", str_schema("英文单词")),
                    ("meaning", str_schema("中文释义")),
                    ("phonetic", str_schema("可选，IPA 音标，不带斜杠")),
                    ("pos", str_schema("可选，词性，如 n./v./adj.")),
                    ("example", str_schema("可选，英文例句")),
                    ("example_zh", str_schema("可选，例句的中文翻译")),
                    (
                        "collocations",
                        array_schema(
                            "可选，常见搭配",
                            schema(
                                &["en", "zh"],
                                &[("en", str_schema("英文搭配")), ("zh", str_schema("中文意思"))],
                            ),
                        ),
                    ),
                ],
            ),
        },
    ]
}

/// The specs the model may see for one session.
pub fn specs_for(skill_editing: bool, phone_tasks: bool) -> Vec<ToolSpec> {
    specs()
        .into_iter()
        .filter(|spec| {
            (phone_tasks || spec.name != PHONE_TASK)
                && (skill_editing || (spec.name != SKILL_READ && spec.name != SKILL_EDIT))
        })
        .collect()
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

fn bool_schema(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

fn array_schema(description: &str, items: Value) -> Value {
    json!({"type": "array", "description": description, "items": items})
}

fn enum_schema(values: &[&str], description: &str) -> Value {
    json!({"type": "string", "description": description, "enum": values})
}

fn object_schema(description: &str) -> Value {
    json!({
        "type": "object",
        "description": description,
        "additionalProperties": {"type": "string"},
    })
}
