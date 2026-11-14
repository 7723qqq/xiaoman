//! The Chinese copy of the Compose screens, one constant per entry of
//! `app/src/main/res/values/strings.xml`, plus the few literals `ChatComponents.kt`,
//! `ChatScreen.kt` and `ApprovalCard.kt` inline instead of using a resource.
//!
//! Keeping the copy here (instead of in the views) means a test can prove the port did not
//! paraphrase the product's wording.

pub const APP_NAME: &str = "小满";
pub const APP_TAGLINE: &str = "你的手机小助手";

// Navigation
pub const NAV_CHAT: &str = "聊天";
pub const NAV_SKILLS: &str = "技能";
pub const NAV_MEMORY: &str = "记忆";
pub const NAV_SETTINGS: &str = "设置";
pub const NAV_SERVICES: &str = "服务";
pub const NAV_VOCAB: &str = "背单词";
pub const BACK: &str = "返回";
pub const CANCEL: &str = "取消";
pub const CLEAR: &str = "清除";

// Session messages added to the transcript
pub const SESSION_NOT_CONFIGURED: &str = "还没有配置模型，请先在设置里填写聊天模型和操作模型。";
pub const SESSION_BUSY: &str = "已有任务在运行，请稍后再试。";
pub const SESSION_STOPPED: &str = "已停止";

// Chat
pub const CHAT_SEND: &str = "发送";
pub const CHAT_STOP: &str = "停止";
pub const CHAT_PLACEHOLDER: &str = "说说你想做什么…";
pub const CHAT_ANSWER_PLACEHOLDER: &str = "告诉我就好…";
pub const CHAT_THINKING: &str = "正在想办法…";
pub const CHAT_WELCOME_TITLE: &str = "今天想做点什么？";
pub const CHAT_WELCOME_BODY: &str = "说一句，我来帮你动手。";
pub const CHAT_WELCOME_FOOTER: &str = "也可以直接输入你的想法";
pub const CHAT_QUESTION_TITLE: &str = "再告诉我一点";
pub const CHAT_QUESTION_TAKEOVER_TITLE: &str = "需要你帮一下";
pub const CHAT_ERROR_TITLE: &str = "这次没完成";
pub const CHAT_ASSISTANT_NAME: &str = "小满";
pub const BANNER_MODELS: &str = "还没有配置聊天模型和操作模型。";
pub const BANNER_MODELS_ACTION: &str = "去设置";

// Status chip of the chat header
pub const STATUS_ATTENTION: &str = "等你确认";
pub const STATUS_BUSY: &str = "处理中";
pub const STATUS_READY: &str = "已就绪";
pub const STATUS_UNCONFIGURED: &str = "待设置";

// Approval card
pub const APPROVAL_TITLE: &str = "这一步，请你确认";
pub const APPROVAL_DENY: &str = "暂不允许";
pub const APPROVAL_ONCE: &str = "允许一次";
pub const APPROVAL_MORE: &str = "更多选项";
pub const APPROVAL_TASK: &str = "本次任务的同类操作都允许";

// Skill draft card
pub const SKILL_DRAFT_TITLE: &str = "技能草稿";
pub const SKILL_DRAFT_CREATE: &str = "新建";
pub const SKILL_DRAFT_UPDATE: &str = "修改";
pub const SKILL_DRAFT_DELETE: &str = "删除";
pub const SKILL_SAVE: &str = "保存";
pub const SKILL_CANCEL: &str = "取消";
pub const SKILL_DRAFT_SHADOW: &str = "将新建用户副本覆盖同名内置技能；内置文件不变，也仍然不能删除";
pub const SKILL_DRAFT_DELETE_WARNING: &str = "将删除用户技能；内置技能不会受影响";

// Process card
pub const PROCESS_LABEL: &str = "处理过程";
pub const PROCESS_EXPAND: &str = "展开处理过程";
pub const PROCESS_COLLAPSE: &str = "收起处理过程";

// Skills
pub const SKILLS_TITLE: &str = "技能";
pub const SKILLS_RELOAD: &str = "重新加载";
pub const SKILLS_PROBLEMS: &str = "加载问题";

// Vocabulary
pub const VOCAB_REFRESH: &str = "刷新";

// Memory
pub const MEMORY_SEARCH_HINT: &str = "搜索记忆";
pub const MEMORY_EMPTY: &str = "还没有记忆。";
pub const MEMORY_NO_HITS: &str = "没有找到相关笔记。";
pub const MEMORY_DELETE: &str = "删除";
pub const MEMORY_DELETE_TITLE: &str = "删除这条笔记？";
pub const MEMORY_DELETE_CONFIRM: &str = "删除";
pub const MEMORY_NOT_FOUND: &str = "这条笔记已不存在。";

// Settings
pub const SETTINGS_TITLE: &str = "设置";
pub const SETTINGS_CHAT_MODEL: &str = "聊天模型";
pub const SETTINGS_OPERATOR_MODEL: &str = "操作模型（需支持图片）";
pub const SETTINGS_BASE_URL: &str = "Base URL";
pub const SETTINGS_MODEL: &str = "模型名称";
pub const SETTINGS_API_KEY: &str = "API Key";
pub const SETTINGS_SHOW_KEY: &str = "显示 API Key";
pub const SETTINGS_HIDE_KEY: &str = "隐藏 API Key";
pub const SETTINGS_TEMPERATURE: &str = "Temperature（可选）";
pub const SETTINGS_AGENT: &str = "任务";
pub const SETTINGS_MAX_STEPS: &str = "最多步数";
pub const SETTINGS_LEARN: &str = "成功后记住操作路径";
pub const SETTINGS_SAVE: &str = "保存";
pub const SETTINGS_SAVED: &str = "已保存";
pub const SETTINGS_INVALID_NUMBER: &str = "请输入有效的数字";
pub const SETTINGS_COPY_CHAT: &str = "与聊天模型相同";
pub const SETTINGS_FOLDERS: &str = "文件夹";

// MCP services
pub const MCP_TITLE: &str = "外部服务";
pub const MCP_INTRO: &str = "勾选后，小满可以直接调用这些服务查询或办事，不用操作手机。会修改数据或产生费用的操作，会先在聊天里问你。";
pub const MCP_RECOMMENDED: &str = "推荐组合";
pub const MCP_TEST: &str = "测试连接";
pub const MCP_TESTING: &str = "正在连接…";
pub const MCP_OFFICIAL: &str = "官方";
pub const MCP_COMMUNITY: &str = "社区";
pub const MCP_APPLY_KEY: &str = "去申请";
pub const MCP_CUSTOM_TITLE: &str = "其他 MCP 服务";
pub const MCP_CUSTOM_HINT: &str =
    "只添加你信任的服务。地址需要是 https，支持 Streamable HTTP 的 MCP 服务都可以接入。";
pub const MCP_CUSTOM_NAME: &str = "名称";
pub const MCP_CUSTOM_URL: &str = "地址（https://…）";
pub const MCP_CUSTOM_AUTH: &str = "Authorization 请求头（可选）";
pub const MCP_CUSTOM_CONFIRM: &str = "调用前确认";
pub const MCP_CONFIRM_WRITES: &str = "会修改数据时确认";
pub const MCP_CONFIRM_ALL: &str = "每次都确认";
pub const MCP_CONFIRM_NONE: &str = "从不确认";
pub const MCP_CUSTOM_ADD: &str = "添加";
pub const MCP_REMOVE: &str = "删除";

/// Skill examples the welcome screen offers, in Compose order.
pub const EXAMPLES: [(&str, &str, &str); 3] = [
    (
        "选一本笔记本",
        "20元以内，先给我确认",
        "帮我在淘宝挑一本20元以内的A5笔记本，选好后给我确认。",
    ),
    ("提醒我喝水", "10分钟后提醒", "10分钟后提醒我喝水。"),
    (
        "记住我的偏好",
        "让助手慢慢懂你",
        "记住我买东西喜欢简单实用。",
    ),
];

/// `stringResource(R.string.skills_empty, path)`.
pub fn skills_empty(folder: &str) -> String {
    format!("还没有技能。把技能文件夹放到 {folder} 后点重新加载。")
}

/// `stringResource(R.string.skills_apps, apps)`.
pub fn skills_apps(apps: &str) -> String {
    format!("App：{apps}")
}

/// `stringResource(R.string.mcp_enabled_count, count)`.
pub fn mcp_enabled_count(count: usize) -> String {
    format!("已启用 {count} 个")
}

/// `stringResource(R.string.mcp_connected, tools)`.
pub fn mcp_connected(tools: usize) -> String {
    format!("已连接，{tools} 个工具")
}

/// `stringResource(R.string.mcp_failed, error)`.
pub fn mcp_failed(error: &str) -> String {
    format!("连接失败：{error}")
}

/// `stringResource(R.string.mcp_missing_key, labels)`.
pub fn mcp_missing_key(labels: &str) -> String {
    format!("还没有填写：{labels}")
}

/// `stringResource(R.string.mcp_custom_invalid, problem)`.
pub fn mcp_custom_invalid(problem: &str) -> String {
    problem.to_string()
}

/// `stringResource(R.string.settings_folder_skills, path)`.
pub fn settings_folder_skills(path: &str) -> String {
    format!("技能：{path}")
}

/// `stringResource(R.string.settings_folder_memory, path)`.
pub fn settings_folder_memory(path: &str) -> String {
    format!("记忆：{path}")
}

/// `stringResource(R.string.settings_folder_traces, path)`.
pub fn settings_folder_traces(path: &str) -> String {
    format!("轨迹：{path}")
}

/// `stringResource(R.string.memory_delete_message, title)`.
pub fn memory_delete_message(title: &str) -> String {
    format!("「{title}」会从手机上永久删除。")
}

/// `"处理过程 · ${entries.size}条记录"` from `ChatComponents.kt`.
pub fn process_header(entries: usize) -> String {
    format!("{PROCESS_LABEL} · {entries}条记录")
}

/// `"流程 ${steps} 步"`: the skills screen's flow indicator.
pub fn skills_flow(steps: usize) -> String {
    format!("流程 {steps} 步")
}
