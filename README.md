# 小满（Xiaoman）

[English](README.en.md)

小满是一个跑在 Android 手机上的开源私人助手。你在它的 App 里聊天，它会记住你告诉它的事，
也能替你操作手机上的其他 App，比如打车、点外卖、改系统设置。新场景以**技能**的形式添加：一个技能
就是一个包含 `SKILL.md` 和可选 `flow.yaml` 的小文件夹，推到手机上即可生效，不需要重新打包 APK。

- 需要 Android 11（API 30）及以上。小满通过无障碍服务读取屏幕并执行操作。
- iOS 无法在手机上单独运行；可用 Mac + WebDriverAgent 的方式试用，见 [docs/ios.md](docs/ios.md)。
- 可接入 MCP 服务（高德地图、腾讯地图、滴滴、12306 等），查询和下单类的事情可以不操作手机，见 [docs/mcp.md](docs/mcp.md)；安卓 App 在「服务」页一键勾选。
- iPhone 不装 App 也能用：网页版添加到主屏幕后聊天、勾选 MCP 服务，见 [docs/web.md](docs/web.md)。
- 模型：任意兼容 OpenAI 接口的服务都可以，例如火山方舟、阿里云百炼、OpenRouter、vLLM、Ollama。
  需要配置两个模型：一个聊天模型，一个能看图的屏幕操作模型。

## 工作方式

```
你：帮我打车回家
 │
 ▼
聊天模型 ── 查记忆（家在哪）、挑技能（taxi-home）
 │
 └─ phone_task ──► 操作循环：截屏 + 节点树 → 操作模型给出一步 → Sentinel 审核 → 执行
                     │                                        │
                     │                         支付 / 发送 / 删除等先问你
                     ▼
               结果摘要回到聊天；成功的操作路径写入记忆，下次执行时参考
```

## 安全

每一步操作在真正落到手机之前，都要经过 Sentinel 策略检查。这一层在模型之外，模型无法绕过：

- **拒绝**：向密码框输入；在受保护（FLAG_SECURE）的页面上操作；技能声明了 `apps` 时操作其他 App。
- **询问**：点击「支付」「发送」「删除」「确认呼叫」等敏感按钮前，需要你确认（允许一次 / 本次任务都允许 / 拒绝）。
- 技能只能**追加**确认词，不能删除内置确认词。
- 每一次决策都会写入审计记录。

## 构建

需要 JDK 17 和 Android SDK 35。

```bash
scripts/gradle.sh --online test :app:assembleDebug   # 首次运行，会下载依赖
scripts/gradle.sh test :app:assembleDebug            # 之后可以离线构建
```

`scripts/gradle.sh` 封装了 `./gradlew`，构建产物放在仓库目录之外，保持工作区干净。可用的环境变量见脚本开头的注释。

## 使用

1. 安装 debug APK，打开小满。
2. 在「设置」里分别填写聊天模型和操作模型的 Base URL、模型名称和 API Key。
3. 按提示开启小满无障碍服务。
4. 回到「聊天」，试试：「帮我打车回家」「记住我家在……」「把 Wi-Fi 打开」。

以火山方舟为例，Base URL 填 `https://ark.cn-beijing.volces.com/api/v3`（也是默认值），模型填方舟上的模型或接入点 ID。

## 课堂实践

[学生动手手册](docs/classroom.md)：TRAE 训练营第二期“用 TRAE 搭建你的个人 Agent 助手”的配套手册。第一节装好小满并跑通淘宝任务，第二节蒸馏你自己，再用 TRAE 改外卖技能、车票技能和首页入口。

## 技能与开发者 CLI

- 技能格式见 [docs/skills.md](docs/skills.md)，示例在 [skills/examples/](skills/examples/)。
- [cli/](cli/README.md)（Python 3，仅依赖标准库）可以校验技能、通过 adb 推送到手机、在 debug 包上直接跑任务并查看轨迹：

```bash
python3 cli/xiaoman.py lint skills/examples/*
python3 cli/xiaoman.py push skills/examples/taxi-home
python3 cli/xiaoman.py run "打开设置里的 Wi-Fi 页面" --skill open-settings-wifi --follow
python3 cli/xiaoman.py traces show <run_id>
```

## Rust 版

同一个产品正在移植到 Rust（`crates/`），Kotlin 版保留为参考实现。当前状态：

- `xiaoman-api` / `llm` / `memory` / `skills` / `sentinel` / `agent` / `mcp`：核心域完整移植，行为与 Kotlin 版逐条对应；`fixtures/` 下的 golden 用例由 Kotlin 与 Rust 测试各跑一遍，逐字节比较。
- `xiaoman-android` + `crates/xiaoman-gpui`：Android 正式前端。无障碍设备层、Session、JNI 和五页 GPUI 界面（聊天、技能、服务、记忆、设置）都在 Rust；Kotlin 只保留无障碍服务、前台服务、通知、AlarmManager 提醒和 JNI 壳。`crates/xiaoman-android/android` 的 Compose 宿主保留为回退。`xiaoman-gpui` 是根 workspace 成员，CI 同时构建两个 APK。
- `xiaoman-app` + `xiaoman-ui`：无头会话与桌面端（GPUI）。
- `xiaoman-web`：网页版，含 `--local` 课堂模式、端口回退、随机访问码和 `/connect` 扫码页；`--max-stdio` 是全局 stdio 子进程上限。
- `xiaoman-ios`：WDA 设备与 Mac 命令行，MCP 工具已接入聊天模型。
- `xiaoman-cli`：Rust 版技能开发 CLI，对应 `cli/` 的 Python 版。

```bash
cargo test --workspace                                   # 含桌面 UI 与 GPUI；GPUI 在桌面平台编译
cargo run -p xiaoman-ui                                  # 桌面版
cargo run -p xiaoman-web --bin web-server -- --local     # 网页版（课堂模式）
./gradlew -p crates/xiaoman-gpui/android assembleDebug   # Android GPUI APK
./gradlew -p crates/xiaoman-android/android assembleDebug # Compose 回退 APK
```

Android 宿主的数据目录是 `/sdcard/Android/data/dev.xiaoman.host/files`，Rust 版 `xiaoman` CLI 默认指向它；调试桥为 `dev.xiaoman.debug.RUN/STOP/RELOAD_SKILLS/SAY`。

## 目录结构

```
xiaoman/
├── core-api/        # 接口与数据结构，其他模块只依赖它
├── core-llm/        # 兼容 OpenAI 的模型客户端
├── core-memory/     # 记忆：Basic Memory 格式的 Markdown 笔记 + 中文检索
├── core-skills/     # 技能加载与校验（SKILL.md / flow.yaml）
├── core-sentinel/   # 安全策略：允许 / 询问 / 拒绝 + 审计
├── core-agent/      # Agent：聊天编排、操作循环、技能流程、经验沉淀
├── hands/           # Android 无障碍层：读屏、点击、输入、悬浮停止按钮
├── ios-hands/、ios-runner/ # iOS WDA 设备接口 / Mac JVM 命令行（使用说明见 docs/ios.md）
├── core-mcp/              # MCP 客户端，把外部服务的工具交给聊天模型（见 docs/mcp.md）
├── mcp/                   # MCP 服务器示例清单 servers.example.json
├── web-server/            # 网页版：服务端 + 移动端网页（见 docs/web.md）
├── crates/          # Rust 移植：api/llm/memory/skills/sentinel/agent/mcp/android/app/ui/web/ios/cli
├── app/             # Android App：聊天、技能、记忆、设置界面
├── cli/             # 技能开发者 CLI
├── skills/examples/ # 示例技能，也会打包进 APK
├── docs/            # 架构与技能文档
└── scripts/         # 构建脚本
```

`core-*` 都是纯 Kotlin/JVM 模块，不依赖 Android API，可以在任何机器上跑单元测试。详细设计见
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## 记忆

记忆是 Basic Memory 格式的 Markdown 文件，存放在
`/sdcard/Android/data/dev.xiaoman.app/files/memory/`，可以直接拷进 Obsidian 查看和编辑。
「忘记」会直接删除对应文件。

## 许可证

MIT，见 [LICENSE](LICENSE)。
