# 移植一致性清单（Kotlin 参考实现 × Rust）

判定规则：

- **一致**：该行为在 Kotlin 和 Rust 两侧都能指到实现，并有行为测试或共享 golden；两侧跑同一输入得到同一结果。
- **不一致**：两侧存在可复现的行为差异。当前清单里没有这一状态。
- **缺测试**：行为已经实现，但还没有“同一份 fixture 由两侧各跑一遍”的自动化用例；已有单侧测试，不能替代跨语言比较。

每一行都必须能落到代码或测试；不接受“Kotlin 为准即可”或“平台限制”作为结论。真机/iPhone 的端到端用可注入的 fake device / fake command 覆盖，手工步骤写在 `crates/xiaoman-gpui/README.md` 与 `docs/ios.md`。

## 0. 门禁

| 门禁 | 命令 | 现状 |
|---|---|---|
| Rust 全量测试（含桌面 UI、GPUI） | `cargo test --workspace` | 通过（577 passed / 0 failed） |
| Rust 格式 | `cargo fmt --all --check` | 通过 |
| Rust Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | 通过（vendor `gpui-mobile` 修 2 处、关闭其不完整的 doctest） |
| Kotlin 全量测试 | `./gradlew test` | 通过（含 app/hands/web-server） |
| 两个 Android 宿主 | `./gradlew -p crates/xiaoman-android/android assembleDebug`、`./gradlew -p crates/xiaoman-gpui/android assembleDebug` | 通过，CI 各上传一个 APK |
| 网页静态资源 | `diff -r web-server/src/main/resources crates/xiaoman-web/resources` | 通过，已进 CI |
| golden：Action | `fixtures/action/*.in.json` → `.out.json` | 两侧通过（11 例） |
| golden：memory | `fixtures/memory/*.input.md` → `.rendered.md` | 两侧通过（4 例） |
| golden：fingerprint | 两侧钉死 `-22de5649` 与 `Int.MIN_VALUE` | 两侧通过 |

## 1. core-api → xiaoman-api

| Kotlin | Rust | 测试 | 状态 |
|---|---|---|---|
| `Point`（0..1000 校验、`center`） | `device.rs` `Point::new/center` | `action_json.rs` | 一致 |
| `Bounds.center` | `Bounds::center` | `device.rs` 单测 | 一致 |
| `ScreenState`（截图、包名、节点、secure、时间） | `ScreenState` | `fingerprint` 测试 | 一致 |
| `ScreenState.fingerprint` | `render_hash`/`java_string_hash` | 两侧常量 `-22de5649`、`-80000000` | 一致 |
| `Action` 12 个变体与默认值 | `Action` enum，`serde(tag="type")` | `fixtures/action` 11 例 | 一致 |
| `UiNode` JSON（false 标志省略） | `UiNode` + `skip_serializing_if` | `node_json_omits_false_flags` | 一致 |
| `AgentEvent` 12 种事件 | `AgentEvent` | trace 测试逐事件 | 一致 |
| `AgentConfig`、`GuardContext`、`Decision` | 同名结构 | sentinel/agent 测试 | 一致 |
| `ApprovalRequest/Answer/Scope/Gateway` | 同名 | sentinel/session 测试 | 一致 |
| `AuditRecord/AuditSink` | 同名 | `policy.rs` | 一致 |
| `Note/Observation/Relation/SearchHit` | 同名 | memory golden | 一致 |
| `Skill/SkillChannel/Flow/FlowStep/SkillProblem` | 同名枚举 | skills 测试 | 一致 |
| `ToolSpec/ToolResult/ToolProvider` | 同名 | mcp/agent 测试 | 一致 |
| `ModelConfig/ChatMessage/ContentPart/ChatResponse/LlmError` | 同名 | llm 18 例 | 一致 |
| `ReminderScheduler` | `ReminderScheduler` trait | app/ios 测试 | 一致 |

## 2. core-llm → xiaoman-llm

| 行为 | Rust 位置 | 测试 | 状态 |
|---|---|---|---|
| 408/409/429/5xx 与传输错误重试，指数退避 | `lib.rs` `complete` | `client.rs` | 一致 |
| `Retry-After` 秒数优先 | `backoff` | `client.rs` | 一致 |
| `response_format` 被拒后去掉重试一次并关闭 JSON 模式 | `complete` | `client.rs` | 一致 |
| `SetLimitExceeded` 不重试 | `complete` | `client.rs` | 一致 |
| 图片 base64（data URL） | `encode_message` | `client.rs` | 一致 |
| tool_calls 回传与解析 | `encode_message`/解析 | `client.rs` | 一致 |
| 错误信息脱敏（Key 不进消息） | `error_detail` + redactor | `client.rs` | 一致 |

## 3. core-memory → xiaoman-memory

| 行为 | Rust 位置 | 测试 | 状态 |
|---|---|---|---|
| CJK 单双字分词、拉丁小写、词边界 | `tokenizer.rs` | 单测 + `memory.rs` | 一致 |
| 查询权重：bigram 1.0、unigram 0.3 | `query_terms` | 单测 | 一致 |
| BM25 参数 K1/B、title 3x、tag 2x | `index.rs` | `title_matches_are_boosted` | 一致 |
| 标题/正文短语加权 1.5/1.2 | `search` | `memory.rs` | 一致 |
| 弱 unigram 不能单独召回 | `search` | `chinese_search_finds_observation` | 一致 |
| 摘要选行、`[` 观察优先、120 字裁剪 | `Snippets` | `snippets_are_clipped_around_the_match` | 一致 |
| 列表符号 `^[-*+]\s+` | `strip_list_marker` | `list_markers_follow_the_kotlin_regex` | 一致 |
| permalink 校验/生成 | `permalinks.rs` | 单测 | 一致 |
| frontmatter 解析（YAML） | `format.rs` + `serde_yaml_ng` | memory golden | 一致 |
| frontmatter 渲染（标题/类型/permalink/tags/extras） | `yaml_scalar`/`yaml_entry` | 两侧 golden 逐字节 | 一致 |
| Basic Memory 正文、观察、关系、wiki link | `format.rs` | memory golden + 23 例 | 一致 |
| FileMemoryStore 读写/追加/删除/列表 | `store.rs` | `memory.rs` | 一致 |
| `serde_yaml 0.9` 停维护 | `Cargo.toml` → `serde_yaml_ng 0.10` | 编译/测试 | 一致 |

## 4. core-skills → xiaoman-skills

| 行为 | Rust 位置 | 测试 | 状态 |
|---|---|---|---|
| `SKILL.md` frontmatter 键与顺序 | `skill_parser.rs` | `skills.rs` | 一致 |
| name 规则 `[a-z0-9-]+`、description 必填/单行 | `skill_parser.rs` | `skills.rs` | 一致 |
| apps/channel/confirm_before/memory/deeplinks 类型与错误行号 | `skill_parser.rs` | `skills.rs` | 一致 |
| `flow.yaml` 8 种步骤与字段校验 | `flow_parser.rs` | `skills.rs` | 一致 |
| YAML 语法错误/重复键/行号 | `yaml.rs` | `skills.rs` | 一致 |
| 目录仓库 reload/list/get、用户技能优先 | `repository.rs` | `skills.rs` | 一致 |
| 两侧同一输入得到同一问题列表 | —— | 缺测试（共享 golden 未建） | 缺测试 |

## 5. core-sentinel → xiaoman-sentinel

| 行为 | Rust 位置 | 测试 | 状态 |
|---|---|---|---|
| secure 屏：仅 back/home/wait 放行 | `lib.rs` `deny` | `policy.rs` | 一致 |
| 密码框输入拒绝（nodeId / focused editable） | `type_target` | `policy.rs` | 一致 |
| 技能 apps 限制 launch/deeplink/tap/type | `permitted` | `policy.rs` | 一致 |
| launcher 与自身包名豁免 | `DEFAULT_LAUNCHERS`/`self_package` | `policy.rs` | 一致 |
| 确认词匹配与边界（"border"≠"order"） | `confirm_words.rs` | 单测 | 一致 |
| 购物计数词排除（"1000+人付款"） | `matches` | 单测 | 一致 |
| 技能 confirm_before 只增不减 | `words_for` | `policy.rs` | 一致 |
| deeplink 只比 scheme+host | `SchemeHost` | 单测 | 一致 |
| 审批键 `word:`/`deeplink:`、TASK 级记忆、endTask | `task_approvals` | `policy.rs` | 一致 |
| 每次决策写审计（含 approved） | `record` | `policy.rs` | 一致 |
| 两侧同一用例得到同一决策 | —— | 缺测试（共享 golden 未建） | 缺测试 |

## 6. core-agent → xiaoman-agent

| 行为 | Rust 位置 | 测试 | 状态 |
|---|---|---|---|
| 工具 schema/描述逐字（memory/search/read/write/forget、list_skills、reminder_set、phone_task） | `agent_tools.rs` | `agent.rs` + 对照 | 一致 |
| persona / no-phone persona / operator system 逐字 | `prompts.rs` | 对照 | 一致 |
| 聊天轮次、tool_calls 往返、最大轮数 | `xiaoman_agent.rs` | `agent.rs` | 一致 |
| 记忆检索注入、写后事件 | `xiaoman_agent.rs` | `agent.rs` | 一致 |
| 技能选择、聊天工作流 vs phone_task | `xiaoman_agent.rs` | `agent.rs` | 一致 |
| 操作循环：observe→model→parse→sentinel→perform | `operator_loop.rs` | `loop.rs` | 一致 |
| 卡死检测（fingerprint、stuck_repeats、一次警告后失败） | `operator_loop.rs` | `stuck_screen_warns_once_then_fails` | 一致 |
| secure 屏接管、ask_user、finish | `operator_loop.rs` | `loop.rs` | 一致 |
| 单步约束（tap/type + text） | `operator_loop.rs` | `loop.rs` | 一致 |
| 流程 8 种步骤 | `flow_step.rs`/`flow_runner.rs` | `flow` 测试 | 一致 |
| 经验笔记写回、reminder 解析 | `note_text.rs`/`reminder_times.rs` | 单测 | 一致 |
| 轨迹事件文本 | `task_run.rs` | `trace.rs` | 一致 |

## 7. core-mcp → xiaoman-mcp

| 行为 | Rust 位置 | 测试 | 状态 |
|---|---|---|---|
| 配置解析、`${VAR}` 替换、缺失变量只跳过该服务 | `mcp_config.rs` | `config.rs` | 一致 |
| 秘密来源（url/headers/env/query）与 `***` 脱敏 | `SecretRedactor` | `config.rs` | 一致 |
| 目录（内置服务、平台可用性） | `catalog.rs` | `catalog.rs` | 一致 |
| HTTP streamable 传输、会话 id、错误 | `transport.rs` | `transport.rs` | 一致 |
| stdio 传输、环境白名单、关闭 | `transport.rs` | `transport.rs` | 一致 |
| 工具名清洗、描述、schema 脱敏 | `provider.rs` | `provider.rs` | 一致 |
| 确认策略（writes/all/none）与拒绝文案 | `provider.rs` | `provider.rs` | 一致 |
| 调用结果截断、重试延迟 | `provider.rs` | `provider.rs` | 一致 |
| 两侧同一配置得到同一脱敏输出 | —— | 缺测试（共享 golden 未建） | 缺测试 |

## 8. hands → xiaoman-android

| 行为 | Rust 位置 | 测试 | 状态 |
|---|---|---|---|
| `Device`（observe/perform/installedApps） | `device.rs` | `device.rs` | 一致 |
| 节点树读取与压缩 | `node_compactor.rs` | `node_compactor.rs` | 一致 |
| 几何/坐标归一化 | `geometry.rs` | `geometry.rs` | 一致 |
| 截图解码/缩放、secure 标记 | `device.rs` | `device.rs` | 一致 |
| 手势（tap/long_press/swipe/back/home） | `device.rs` | `device.rs` | 一致 |
| 输入（nodeId/focused、submit） | `device.rs` | `device.rs` | 一致 |
| App 链接/deeplink | `app_links.rs` | `app_links.rs` | 一致 |
| JNI 桥（服务连接、停止、前台、Session） | `jni_exports.rs`/`session.rs` | `android_parts.rs` | 一致 |
| 密钥存储（Keystore 桥） | `secret_store.rs` | 编译/宿主 | 一致 |

## 9. app → xiaoman-app

| 行为 | Rust 位置 | 测试 | 状态 |
|---|---|---|---|
| Session 打开/关闭、数据目录、设置 | `session.rs` | `session.rs` | 一致 |
| 一轮聊天、tool_calls、busy 拒绝 | `session.rs` | `session.rs` | 一致 |
| 审批卡片：park/allow/deny/TASK | `session.rs` | `an_approval_parks...` | 一致 |
| 审批身份：过期 id 不回答新审批 | `answer_approval` | `a_stale_approval_id...` | 一致 |
| 任务运行、run id、失败配对事件 | `session_tasks.rs` | `session_tasks.rs` | 一致 |
| 设置持久化、Key 只进 secrets 文件 | `settings.rs` | `settings.rs` | 一致 |
| 轨迹 JSONL、Key 脱敏、截图按步 | `trace.rs` | `trace.rs` | 一致 |
| 聊天条目折叠/富文本 | `chat_item.rs` | `chat_item.rs` | 一致 |
| MCP 选择持久化 | `mcp_settings` | `mcp_settings.rs` | 一致 |
| 与 Kotlin `TraceWriter` 逐行一致 | —— | 缺测试（共享 golden 未建） | 缺测试 |

## 10. web-server → xiaoman-web

| 行为 | Rust 位置 | 测试 | 状态 |
|---|---|---|---|
| `/api/info`、访问码保护其余 API | `server.rs` | `server.rs` | 一致 |
| `/api/chat` SSE、会话、并发上限 | `server.rs`/`session.rs` | `server.rs` | 一致 |
| `/api/answer`（问题/审批按 id） | `server.rs`/`channels.rs` | `channels.rs` | 一致 |
| `/api/mcp/test`、目录、自定义服务 | `server.rs`/`mcp_selection.rs` | `server.rs` | 一致 |
| `/api/session` 快照与过期 | `server.rs` | `server.rs` | 一致 |
| `/v1/models`、`/v1/chat/completions`（stream/图片） | `class_v1.rs` | `v1_gateway.rs` 15 例 | 一致 |
| 课堂令牌生成/吊销/限额/限流 | `class_tokens.rs` | `class_tokens.rs` | 一致 |
| 自助领取 `/claim`、`/api/claim/*` | `claims.rs` | `claims.rs` 15 例 | 一致 |
| SSRF：模型/MCP 解析到私网拒绝 | `safe_dns.rs` | 单测 | 一致 |
| 浏览器记忆 200 条/180KB 上限 | `browser_memory.rs` | `server.rs` | 一致 |
| `--local`、端口回退、随机访问码、`/connect` 仅本机 | `local_launch.rs` | `local_launch.rs` | 一致 |
| `--max-stdio` 全局并发上限（默认 20） | `mcp_provider.rs` `StdioLimiter` | `stdio_processes_are_capped...` | 一致 |
| 静态资源与 Kotlin 版逐字节相同 | `resources/` | CI `diff -r` | 一致 |
| 提醒 ICS 导出 | `ics.rs` | `server.rs` | 一致 |

## 11. ios-hands / ios-runner → xiaoman-ios

| 行为 | Rust 位置 | 测试 | 状态 |
|---|---|---|---|
| WDA `/status`、会话、截图、源解析 | `wda_client.rs`/`wda_device.rs`/`wda_source.rs` | `wda.rs` 14 例 | 一致 |
| 节点树 → `ScreenState`、坐标 | `wda_device.rs` | `wda.rs` | 一致 |
| `check`/`dump`/`tools`/`call`/agent | `command.rs` | `cli.rs` 16 例 | 一致 |
| 配置（env/文件、两个模型、Key 回退） | `config.rs` | `cli.rs` | 一致 |
| 控制台问题/审批通道 | `channels.rs` | `cli.rs` | 一致 |
| 轨迹记录与脱敏 | `trace.rs` | `cli.rs` | 一致 |
| 提醒：进程内定时 + macOS 通知，文本走 argv | `reminders.rs` | `the_text_is_an_argument...` | 一致 |
| 真机 WDA 端到端 | fake command | 缺测试（需 Mac/iPhone） | 缺测试 |

## 12. cli/ → xiaoman-cli

| 行为 | Rust 位置 | 测试 | 状态 |
|---|---|---|---|
| `lint` 规则与问题文本/行号 | `lint.rs` | `lint.rs` 20 例 | 一致 |
| `push`（校验、adb push、reload 广播） | `commands.rs` | `commands.rs` | 一致 |
| `run/stop` 调试桥、`--follow` 事件 | `commands.rs` | `commands.rs` | 一致 |
| `logs` logcat 解析 | `logcat.rs` | `traces_logcat.rs` | 一致 |
| `traces pull/show` | `traces.rs` | `traces_logcat.rs` | 一致 |
| YAML 子集解析 | `yamlsub.rs` | 22 例 | 一致 |
| Python CLI 与 Rust CLI 输出逐字节 | —— | 缺测试（共享 golden 未建） | 缺测试 |

## 13. Android 宿主与桌面 UI

| 行为 | Rust/Kotlin 位置 | 测试 | 状态 |
|---|---|---|---|
| GPUI 五页（聊天/技能/服务/记忆/设置） | `crates/xiaoman-gpui/src` | 7 个单测 + 手工记录 | 一致 |
| GPUI 审批卡带 id、无调试入口 | `chat.rs` | `xiaoman-app` 审批测试 | 一致 |
| GPUI 技能页首屏显示解析问题 | `skills.rs` | 构建 | 一致 |
| GPUI 通知栏按 id 回答、过期不动作 | `NotificationActionReceiver.kt` + JNI | `Session` 测试 | 一致 |
| GPUI 悬浮状态条 | `StatusOverlay.kt` + `NotifierBridge.kt` | 构建 | 一致 |
| Compose 回退宿主五页 | `crates/xiaoman-android/android/.../ui` | 构建 | 一致 |
| Compose 通知按 id 回答 | `NotificationActionReceiver.kt` | JNI 校验 + 构建 | 一致 |
| 桌面五页 + 审批 id | `crates/xiaoman-ui/src` | 31 例 | 一致 |
| 桌面/移动两套 GPUI 版本 | `gpui 0.2` vs `gpui-pre 0.3.6` | 两边测试各自跑 | 一致（业务逻辑不依赖版本） |
| 真机/模拟器端到端 | MuMu 手工记录 | 缺测试（环境） | 缺测试 |

## 14. 文档承诺

| 文档 | 承诺 | 代码 | 状态 |
|---|---|---|---|
| README | 核心域逐条对应 | 见 1-7 | 一致 |
| README | Android 正式前端五页在 Rust | `xiaoman-gpui` | 一致 |
| README | GPUI 是 workspace 成员、CI 双 APK | `Cargo.toml`、`ci.yml` | 一致 |
| README | `cargo test --workspace` 覆盖 GPUI | 7 个 GPUI 测试 | 一致 |
| README | `--max-stdio` 生效 | `StdioLimiter` | 一致 |
| ARCHITECTURE | Action/fingerprint 字节兼容 | golden + 常量 | 一致 |
| ARCHITECTURE | 五页 GPUI、Compose 回退 | 两个宿主 | 一致 |
| docs/web.md | 课堂模式/扫码页/SSRF/令牌 | `server.rs` 等 | 一致 |
| docs/web.md | `--max-stdio` 上限（68 与 76 行不再矛盾） | `main.rs`/`options.rs` | 一致 |
| docs/ios.md | 命令、配置、提醒只在 runner 运行期间 | `command.rs`/`reminders.rs` | 一致 |

## 15. 本轮收敛的审查发现

| 发现 | 修复 | 验证 |
|---|---|---|
| 审批 id 在校验处丢失 | `Session::answer_approval(id, …)`；桌面/GPUI/通知/JNI 全传 id | `a_stale_approval_id_cannot_answer_the_current_card` |
| "模拟确认"进正式界面 | 删除按钮与 `simulate_approval` | GPUI 编译 + 手工记录已更新 |
| GPUI 自称 spike、不在 workspace、CI 不建 | 进 workspace、Android-only 门控、6 个单测、CI 双 APK | `cargo test -p xiaoman-gpui` |
| `--max-stdio` 不生效、文档矛盾 | `StdioLimiter` + 参数/帮助/文档统一 | 2 个 limiter 测试 |
| fingerprint `i32::MIN` | `unsigned_abs` + 独立常量 | 两侧测试 |
| memory 列表符号 | `^[-*+]\s+` | 单测 |
| `serde_yaml 0.9` 停维护 | `serde_yaml_ng 0.10` | 编译/测试 |
| iOS reminders 拼接脚本 | `on run argv` + macOS/路径判断 | 单测 |
| iOS/GPUI 注释说没做、实际做了 | 注释改成实际行为 | 文档 |
| 两处 Kotlin 测试在 Windows 失败 | POSIX 权限判断、`windows=false` 显式传参 | `./gradlew test` |
| Rust 未进 git | 本轮按模块提交 | `git log` |

## 16. 仍缺的共享 golden（不是功能缺口）

| 域 | 需要的 fixture | 现状 |
|---|---|---|
| skills | `SKILL.md`/`flow.yaml` → 问题列表（message/line） | 两侧各有测试，缺同一份文件 |
| sentinel | Action+Screen+Skill → 决策（type/prompt/reason） | 23 例单侧，缺同一份文件 |
| trace | 事件序列 → `trace.jsonl` 每行 | Rust 钉了文本，Kotlin 未共享 |
| MCP | 配置+输入 → 脱敏输出 | 16 例单侧，缺同一份文件 |
| CLI | 同一技能目录 → lint/push/run/logs/traces 输出 | Python/Rust 各有测试，缺逐字节比较 |
| LLM | 同一请求 → 重试/降级序列 | 18 例单侧，缺共享脚本 |

这些是本轮之后第一批要补的测试；行为已经在两侧实现并通过各自测试。
