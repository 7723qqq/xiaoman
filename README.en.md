# Xiaoman

[中文](README.md)

An open-source personal agent that lives on an Android phone. You chat with it in its app; it
remembers what you tell it and operates the other apps on the phone for you. New scenarios are
**skills**: small `SKILL.md` + `flow.yaml` folders you can push without rebuilding the APK, or
create, edit and delete from the chat after confirming a card.

- Android 11 (API 30) or newer. It uses an accessibility service to read the screen and act.
- iOS cannot run it on the phone alone. You can try it from a Mac that drives the iPhone through
  WebDriverAgent; see [docs/ios.md](docs/ios.md) (Chinese).
- MCP servers (Amap, Tencent Maps, DiDi, 12306 ...) can serve lookups and bookings without operating
  the phone; see [docs/mcp.md](docs/mcp.md) (Chinese).
- A web version (add to the iPhone home screen) offers chat and one-tap MCP services without an app;
  see [docs/web.md](docs/web.md) (Chinese).
- Models: any OpenAI-compatible endpoint. You configure a chat model and a screen ("operator")
  model that accepts images.

Everything is implemented in Rust under `crates/`. The little Kotlin left in the Android host only
does what Android requires from Kotlin/JVM: the accessibility service, the foreground service,
notifications, AlarmManager reminders, Keystore, the status overlay and the JNI shell.

## Safety

Every action passes the Sentinel policy before it reaches the phone, outside the model's control:
typing into password fields and acting on secure (FLAG_SECURE) screens are denied; taps on things
like 支付 / 发送 / 删除 / 确认呼叫 need your approval; a skill may only operate the apps it lists.
Skills can add confirmation words but never remove the built-in ones. Every decision is audited.

Skill editing has its own card: the model can only propose a draft, the chat shows the full
`SKILL.md`, and nothing is written until you press 保存. Bundled skills cannot be deleted, and
content that looks like a phone number, password or API key is rejected before the card appears.

## Build

Rust; the Android APK also needs JDK 17, Android SDK 35 and NDK 27.

```bash
cargo test --workspace
cargo run -p xiaoman-ui
cargo run -p xiaoman-web --bin web-server -- --local
./gradlew -p crates/xiaoman-gpui/android assembleDebug
```

`gradlew` is the entry point for the Android host (`crates/xiaoman-gpui/android` is a standalone
Gradle project). Point at the NDK with `-Pxiaoman.ndkDir=...`, `ANDROID_NDK_HOME` or
`ANDROID_NDK_ROOT`.

## Use

1. Install the debug APK and open Xiaoman.
2. Settings: enter base URL, model and API key for the chat and operator models.
3. Turn on the Xiaoman accessibility service when the app asks.
4. Chat: "帮我打车回家", "记住我家在 ...", "把 Wi-Fi 打开".

## Skills and developer CLI

- Format: [docs/skills.md](docs/skills.md); examples in [skills/examples/](skills/examples/).
- The Rust `xiaoman-cli` lints skills, pushes them over adb, runs tasks on a debug build and shows
  traces:

```bash
cargo run -p xiaoman-cli -- lint skills/examples/*/
cargo run -p xiaoman-cli -- push skills/examples/taxi-home
cargo run -p xiaoman-cli -- run "打开设置里的 Wi-Fi 页面" --skill open-settings-wifi --follow
cargo run -p xiaoman-cli -- traces show <run_id>
```

## Layout

`crates/xiaoman-api` (contracts), `xiaoman-llm`, `xiaoman-memory`, `xiaoman-skills`,
`xiaoman-vocab` (vocabulary engine), `xiaoman-sentinel`, `xiaoman-agent`, `xiaoman-mcp`,
`xiaoman-android`, `xiaoman-gpui` (Android
GPUI front-end), `xiaoman-app` (headless session), `xiaoman-ui` (desktop), `xiaoman-web`,
`xiaoman-ios`, `xiaoman-cli`.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) (Chinese).

## Memory

Notes are Markdown files in the Basic Memory format under
`/sdcard/Android/data/dev.xiaoman.app/files/memory/`, so you can read and edit them in Obsidian.
Forgetting deletes the file.

## License

MIT, see [LICENSE](LICENSE).
