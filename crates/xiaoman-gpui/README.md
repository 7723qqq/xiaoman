# xiaoman-gpui（Android 前端）

Android 的正式前端：GPUI + [`gpui-mobile`](https://github.com/itsbalamurali/gpui-mobile)，五页
（聊天 / 技能 / 服务 / 记忆 / 设置）都是 Rust。Kotlin 只保留 Android 不能交给 Rust 的对象：无障碍服务、
前台服务、通知、AlarmManager、debug 广播和 JNI 壳。`crates/xiaoman-android/android` 的 Compose 宿主
保留为回退实现。

它**不在根 workspace 里**（gpui-mobile 会拉 wgpu/cosmic-text），`cargo test --workspace` 不会构建它；
用 `android/` 的 Gradle 工程出 APK。

## 组成

- `src/lib.rs`：`android_main`，初始化 `AndroidApp`/GPUI 平台，复用 `HostApplication` 通过 JNI 打开的
  `xiaoman_android::session`。
- `src/app.rs`：底部五页导航，对应原 Compose 的五个 tab。
- `src/chat.rs`：折叠的对话时间线（`处理过程` 可展开）、输入框（`gpui-kit` 的 `Input`，走
  `GpuiInputActivity` 的 InputConnection）、审批卡、停止，以及通知/前台服务的镜像。
- `src/settings.rs`：两个模型、步数、学习开关，以及宿主状态（无障碍/前台服务/Rust 库/通知权限）。
- `src/skills.rs`、`services.rs`、`memory.rs`：技能、MCP 目录与自定义服务、记忆的搜索/打开/删除。
- `vendor/gpui-mobile`：上游 `gpui-mobile` 源码（Apache-2.0 / GPL-3.0 / AGPL-3.0 三选一，这里按
  Apache-2.0 使用；见其 `LICENSE-APACHE` 与 `README.md`）。
- `android/`：NativeActivity 宿主 + 从 Compose 宿主复用的 Kotlin 服务（HostApplication、AgentService、
  Notifications、AlarmReminders、DebugReceiver、无障碍服务），`android.app.lib_name=xiaoman_gpui`，
  仅 arm64-v8a。`GpuiInputActivity` 来自 gpui-mobile 的示例，提供中文输入法的多段组合。

## 与原 Kotlin App 对齐的部分

- 同一份 `xiaoman_app::Session`：聊天、审批、任务、记忆、技能、MCP 全部走 Rust。
- 调试桥 `dev.xiaoman.debug.RUN/STOP/RELOAD_SKILLS/SAY`，轨迹在
  `/sdcard/Android/data/dev.xiaoman.host/files/traces/<run_id>/`，包含 `trace.jsonl`、`audit.jsonl`
  和每步截图 `step-<n>.png`（`RecordingDevice`）。
- 审批/提问通知（允许一次、本次都允许、暂不允许），提醒走 AlarmManager，前台服务显示状态并带停止。
- 内置示例技能从 `assets/skills` 释放；数据目录和 CLI 的包名一致（`dev.xiaoman.host`）。
- 原 Compose 的 `处理过程` 折叠、宿主状态、通知权限都在 GPUI 里复现。

## 构建

```bash
export ANDROID_NDK_HOME="$HOME/AppData/Local/Android/Sdk/ndk/27.2.12479018"
export JAVA_HOME="/c/Program Files/Eclipse Adoptium/jdk-17.0.20.8-hotspot"
./gradlew -p crates/xiaoman-gpui/android --offline assembleDebug
```

Gradle 的 `cargoBuild` 任务会执行：

```bash
cargo build --release --target aarch64-linux-android \
  --manifest-path crates/xiaoman-gpui/Cargo.toml --target-dir target
```

并把 `libxiaoman_gpui.so` 放进 `app/src/main/jniLibs/arm64-v8a/`。

## 已知限制

- 只验证了 arm64（MuMu 用 arm64 兼容层跑）；gpui-mobile 的 x86_64/armv7 尚未测试。
- 桌面 `xiaoman-ui` 仍在 `gpui 0.2`，与这里的 `gpui-pre 0.3.6` 不是同一 API；两套视图暂未合并。
  视图模型的行为由 `xiaoman-ui/tests/view_model.rs` 和本 crate 的单元测试分别覆盖，两个 GPUI
  版本都不参与业务决策，因此不改变聊天、审批、记忆、技能和 MCP 的行为。
- `android-activity` 会周期性打印 `Spurious ALOOPER_POLL_CALLBACK`，不影响运行但日志很吵。


## 2026-11-13 实测（MuMu，Android 15）

- 环境：MuMu 12，Android 15（API 35），x86_64 + arm64 兼容层，Vulkan/Adreno 640。
- APK 安装并启动成功；GPUI 选中 Vulkan 适配器，窗口 1920×1080，中文正常显示。
- 聊天页、输入框、发送、停止、审批卡全部渲染；`session.send("1")` 后出现真实的
  “还没有配置模型，请先在设置里填写聊天模型和操作模型。”
- 审批卡由 `Session` 的 pending approval 驱动；正式构建里没有调试入口，过期卡片或通知里的
  allow/deny 会被 `Session` 按 approval id 拒绝（见 `xiaoman-app` 的
  `a_stale_approval_id_cannot_answer_the_current_card`）。
- 已验证（用户实测）：模拟器软键盘可以输入中文，`GpuiInputActivity` 的 InputConnection
  路径可用。`adb shell input text/keyevent` 注入的字符不会经过输入法，因此进不了 GPUI 输入框；
  这是测试手段的限制，不是应用缺陷。
- android-activity 会周期性打印 `Spurious ALOOPER_POLL_CALLBACK`，不影响运行，但日志很吵。
- `dev.gpui.mobile.GpuiPlatformView` 类缺失的日志来自 gpui-mobile 的可选平台视图功能，本 UI 不用。
