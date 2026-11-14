# 小满（Xiaoman）

[English](README.en.md)

小满是一个跑在 Android 手机上的开源私人助手。你在它的 App 里聊天，它会记住你告诉它的事，
也能替你操作手机上的其他 App，比如打车、点外卖、改系统设置。新场景以**技能**的形式添加：一个技能
就是一个包含 `SKILL.md` 和可选 `flow.yaml` 的小文件夹，不需要重新打包 APK；也可以直接在聊天里
让小满新建、修改或删除技能，确认后才保存。

- 需要 Android 11（API 30）及以上。小满通过无障碍服务读取屏幕并执行操作。
- iOS 无法在手机上单独运行；可用 Mac + WebDriverAgent 的方式试用，见 [docs/ios.md](docs/ios.md)。
- 可接入 MCP 服务（高德地图、腾讯地图、滴滴、12306 等），查询和下单类的事情可以不操作手机，见 [docs/mcp.md](docs/mcp.md)；安卓 App 在「服务」页一键勾选。
- iPhone 不装 App 也能用：网页版添加到主屏幕后聊天、勾选 MCP 服务，见 [docs/web.md](docs/web.md)。
- 模型：任意兼容 OpenAI 接口的服务都可以，例如火山方舟、阿里云百炼、OpenRouter、vLLM、Ollama。
  需要配置两个模型：一个聊天模型，一个能看图的屏幕操作模型。

整个项目只有 Rust 实现（`crates/`）。Android 宿主里保留的少量 Kotlin 只做 Android 框架必须由
Kotlin/JVM 完成的事：无障碍服务、前台服务、通知、AlarmManager、Keystore、悬浮窗和 JNI 壳。

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
- 技能编辑：模型只能提交草稿，聊天页弹出卡片展示完整 `SKILL.md`，点「保存」才写盘；内置技能不能删除；
  正文包含手机号、密码、Key 形状时会直接拒绝。

## 构建

需要 Rust 工具链；Android APK 还需要 JDK 17、Android SDK 35 和 NDK 27。

```bash
cargo test --workspace                                   # 全部 Rust 测试
cargo run -p xiaoman-ui                                  # 桌面版
cargo run -p xiaoman-web --bin web-server -- --local     # 网页版（课堂模式）
./gradlew -p crates/xiaoman-gpui/android assembleDebug   # Android APK
```

`gradlew` 是 Android 宿主的构建入口（`crates/xiaoman-gpui/android` 是独立 Gradle 工程）。NDK 路径用
`-Pxiaoman.ndkDir=...`、`ANDROID_NDK_HOME` 或 `ANDROID_NDK_ROOT` 指定。

## 使用

1. 安装 debug APK，打开小满。
2. 在「设置」里分别填写聊天模型和操作模型的 Base URL、模型名称和 API Key。
3. 按提示开启小满无障碍服务。
4. 回到「聊天」，试试：「帮我打车回家」「记住我家在……」「把 Wi-Fi 打开」。

以火山方舟为例，Base URL 填 `https://ark.cn-beijing.volces.com/api/v3`（也是默认值），模型填方舟上的模型或接入点 ID。

## 技能与开发者 CLI

- 技能格式见 [docs/skills.md](docs/skills.md)，示例在 [skills/examples/](skills/examples/)。
- 开发者 CLI 是 Rust 的 `xiaoman-cli`，可以校验技能、通过 adb 推送到手机、在 debug 包上直接跑任务并查看轨迹：

```bash
cargo run -p xiaoman-cli -- lint skills/examples/*/
cargo run -p xiaoman-cli -- push skills/examples/taxi-home
cargo run -p xiaoman-cli -- run "打开设置里的 Wi-Fi 页面" --skill open-settings-wifi --follow
cargo run -p xiaoman-cli -- traces show <run_id>
```

## 目录结构

```
xiaoman/
├── crates/
│   ├── xiaoman-api/       # 接口与数据结构
│   ├── xiaoman-llm/       # 兼容 OpenAI 的模型客户端
│   ├── xiaoman-memory/    # 记忆：Basic Memory 格式 + 中文检索
│   ├── xiaoman-skills/    # 技能解析、仓库与写入器
│   ├── xiaoman-vocab/     # 背单词引擎：内置专升本词库 + 艾宾浩斯复习 + 提醒
│   ├── xiaoman-sentinel/  # 安全策略：允许 / 询问 / 拒绝 + 审计
│   ├── xiaoman-agent/     # Agent：聊天编排、操作循环、技能流程、经验沉淀
│   ├── xiaoman-mcp/       # MCP 客户端
│   ├── xiaoman-android/   # Android 设备层与 JNI
│   ├── xiaoman-gpui/      # Android 正式前端（GPUI）
│   ├── xiaoman-app/       # 无头会话
│   ├── xiaoman-ui/        # 桌面端
│   ├── xiaoman-web/       # 网页版服务端
│   ├── xiaoman-ios/       # iOS WDA 设备与 Mac runner
│   └── xiaoman-cli/       # 技能开发者 CLI
├── fixtures/          # Rust 测试用的固定输入
├── mcp/               # MCP 服务器示例清单
├── skills/examples/   # 示例技能，也会打包进 APK
├── docs/              # 架构与技能文档
├── scripts/ios/       # WDA 安装工具
└── deploy/            # 网页版部署脚本
```

详细设计见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## 记忆

记忆是 Basic Memory 格式的 Markdown 文件，存放在
`/sdcard/Android/data/dev.xiaoman.app/files/memory/`，可以直接拷进 Obsidian 查看和编辑。
「忘记」会直接删除对应文件。

## 课堂实践

[学生动手手册](docs/classroom.md)：TRAE 训练营第二期“用 TRAE 搭建你的个人 Agent 助手”的配套手册。

## 许可证

MIT，见 [LICENSE](LICENSE)。
