# Android host for `crates/xiaoman-android`

The JVM shell around `libxiaoman_android.so`. Kotlin keeps only the Android objects — the
`AccessibilityService`, the `specialUse` foreground service with its notification, and one launcher
screen — and Rust keeps every decision: the operator loop, the node compactor, screenshots, gestures
and the policy.

The two halves are joined by the tables in `crates/xiaoman-android/src/bridge.rs`. This directory is
its own Gradle build: it is not included by the repository's root `settings.gradle.kts`, and the
existing `app/` and `hands/` modules are untouched.

## Files

| Path | What it is |
| --- | --- |
| `settings.gradle.kts`, `build.gradle.kts`, `gradle.properties` | Standalone application build; the root project *is* the app module |
| `src/main/AndroidManifest.xml` | Accessibility service, foreground service with `specialUse` subtype property, launcher activity, package visibility `<queries>` |
| `src/main/res/xml/xiaoman_accessibility.xml` | Accessibility config: gestures, window content, screenshots, interactive windows, view ids |
| `src/main/res/values/strings.xml` | Chinese copy from the existing app plus the host screen's strings |
| `src/main/res/values/{colors,themes}.xml`, `drawable/`, `mipmap-anydpi-v26/` | Icon, notification icon, framework theme |
| `src/main/kotlin/dev/xiaoman/hands/XiaomanAccessibilityService.kt` | Every row of `SERVICE_METHODS`, the node-handle table, and the `SERVICE_NATIVES` callbacks |
| `src/main/kotlin/dev/xiaoman/hands/ScreenShot.kt` | What `captureScreen()` returns: `getPng(): ByteArray?` and `getSecure(): boolean` |
| `src/main/kotlin/dev/xiaoman/hands/NativeLibrary.kt` | `System.loadLibrary("xiaoman_android")`, once, with the outcome remembered |
| `src/main/kotlin/dev/xiaoman/app/AgentService.kt` | `FOREGROUND_SERVICE_METHODS` as static methods, plus the `FOREGROUND_SERVICE_NATIVES` stop callback |
| `src/main/kotlin/dev/xiaoman/app/MainActivity.kt` | Minimal host screen: what is on, and the buttons that switch it on |

### Which class implements what

All 24 `SERVICE_METHODS` are `dev.xiaoman.hands.XiaomanAccessibilityService` (non-static, called on
the service object Rust holds as a global reference):

`isConnected`, `displayWidthPx`, `displayHeightPx`, `foregroundPackage`, `foregroundActivity`,
`captureScreen`, `readRoots`, `nodeFactsJson`, `nodeChildCount`, `nodeChild`, `nodeRelease`,
`nodeRefresh`, `nodeClick`, `nodeLongClick`, `nodeSetText`, `nodeImeEnter`, `editableFocus`,
`dispatchStroke`, `globalBack`, `globalHome`, `launch`, `openUri`, `startAlarm`, `installedAppsJson`.

`SERVICE_NATIVES` (Java → Rust, exported as
`Java_dev_xiaoman_hands_XiaomanAccessibilityService_<name>`): `nativeOnServiceConnected`,
`nativeOnServiceUnbound`, `nativeRequestStop`, `nativeOnForeground`.

All 4 `FOREGROUND_SERVICE_METHODS` are `@JvmStatic` members of `dev.xiaoman.app.AgentService`'s
companion object, so `env.call_static_method` resolves them:
`start(Context): Boolean`, `stop(Context): Unit`, `isRunning(): Boolean`,
`updateStatus(Context, String): Unit`.

`FOREGROUND_SERVICE_NATIVES`: `nativeRequestStop`, declared as an **instance** native on
`AgentService` — its second JNI argument is the service (`jobject`), not the class. Only that name
must appear, not an overload.

## Prerequisites

Exact versions the build expects:

- **JDK 17**. `jvmToolchain(17)` in `build.gradle.kts` and `sourceCompatibility`/`targetCompatibility`
  are 17, and AGP 8.7.3 requires 17. `JAVA_HOME` must point at a 17 JDK — on this machine `java` on
  `PATH` is 1.8 while a 17 JDK is installed under `Program Files/Eclipse Adoptium`, which is not
  enough.
- **Android SDK, platform 35** (`compileSdk = 35`, `targetSdk = 35`) with `build-tools` for that
  platform, plus `platform-tools` for `adb`. `ANDROID_HOME` (or `ANDROID_SDK_ROOT`, or a
  `local.properties` with `sdk.dir=…`) must point at it. An SDK directory without `platforms/` and
  `build-tools/` cannot build this: AGP needs both.
- **Android NDK r26 or newer**, e.g. **27.2.12479018 (r27c)**, which is what is installed here.
  Set `ANDROID_NDK_HOME` (or `ANDROID_NDK_ROOT`, or `-Pxiaoman.ndkDir=<path>`). The task uses it for
  the `cargo rustc` linker; the APK's `minSdk` is 30, so the NDK's `*-android30-clang` wrappers are
  the ones it points at.
- **rustup targets** for every ABI in `xiaoman.abis` (both are the default):

  ```sh
  rustup target add aarch64-linux-android x86_64-linux-android
  ```

- **No cargo-ndk.** `cargoNdk` runs `cargo build --release --target <triple>` and points Cargo at
  the NDK's clang wrapper with `CARGO_TARGET_<TRIPLE>_LINKER` (and `CC_<triple>`):
  `<ndk>/toolchains/llvm/prebuilt/<host>/bin/<abi><api>-clang`, where `<api>` is `minSdk` (30).
  That works because `crates/xiaoman-android/Cargo.toml` declares
  `[lib] crate-type = ["cdylib", "rlib"]` — `cdylib` is the `.so`, `rlib` keeps `cargo test` and
  the crate's integration tests building on the host.

- **Plugin versions are pinned to what this machine's Gradle cache holds**: AGP 8.9.2, Kotlin
  2.0.21, `androidx.core:core-ktx` 1.13.1. The repository's root catalog asks for AGP 8.7.3 /
  Kotlin 2.1.0 / core-ktx 1.15.0, which resolve fine with network access but take a long time here
  (Maven Central measured at ~35 KB/s). Raise them back if the cache is populated.

## Build, in this order

From the repository root. The Gradle wrapper lives there; `-p` makes this directory the project.

```sh
# 0. one time, per machine
export JAVA_HOME="/c/Program Files/Eclipse Adoptium/jdk-17.0.20.8-hotspot"   # a JDK 17
export ANDROID_HOME="$HOME/AppData/Local/Android/Sdk"                        # platform 35 + build-tools
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.2.12479018"
rustup target add aarch64-linux-android x86_64-linux-android

# 1. build libxiaoman_android.so and copy it into jniLibs
./gradlew -p crates/xiaoman-android/android cargoNdk

# 2. build the APK (preBuild depends on cargoNdk, so step 1 also runs from here)
./gradlew -p crates/xiaoman-android/android assembleDebug

# 3. install and run
adb install -r crates/xiaoman-android/android/build/outputs/apk/debug/*.apk
adb shell am start -n dev.xiaoman.host/dev.xiaoman.app.MainActivity
```

Step 1 is the one that needs the NDK and the rustup targets; step 2 only needs the JDK and SDK once
`build/rustJniLibs/<abi>/libxiaoman_android.so` exists. Running Gradle from inside this directory
works the same way:

```sh
cd crates/xiaoman-android/android
../../../gradlew cargoNdk
../../../gradlew assembleDebug
```

If no NDK is exported, pass it explicitly — a Gradle property is read before the environment:

```sh
./gradlew -p crates/xiaoman-android/android assembleDebug \
  -Pxiaoman.ndkDir="C:\\Users\\me\\AppData\\Local\\Android\\Sdk\\ndk\\27.2.12479018"
```

If `./gradlew` cannot fetch its distribution (it wants `gradle-8.11.1-bin.zip` from
`services.gradle.org`, which times out on this machine), run the same project with an installed
Gradle: any Gradle 8.11.1+ works, for example the one the wrapper already extracted under
`~/.gradle/wrapper/dists/gradle-8.13-bin/*/gradle-8.13/bin/gradle`.

After installing, the accessibility service has to be enabled by hand, in either place: the host
screen's “去开启无障碍服务” button, or Settings → Accessibility → 小满. No app can enable it for
itself.

### Gradle properties

| Property | Default | Meaning |
| --- | --- | --- |
| `xiaoman.abis` | `arm64-v8a,x86_64` | Comma separated ABI list to build and package (the emulator is x86_64) |
| `xiaoman.rust.skip` | `false` | `true` assembles a Kotlin-only APK; every native call logs and returns false |
| `xiaoman.cargoTargetDir` | `<repo>/target` | Where cargo writes artifacts; the same directory `cargo build` uses |
| `xiaoman.ndkDir` | unset | NDK for the linker when `cargo-ndk` is absent |

## How the library is loaded

`cargoNdk` writes `build/rustJniLibs/<abi>/libxiaoman_android.so`, and `build.gradle.kts` adds that
directory to `jniLibs`:

```kotlin
sourceSets.getByName("main") { jniLibs.srcDir(rustJniLibsDir.get().asFile) }
```

So `System.loadLibrary("xiaoman_android")` — called once from `NativeLibrary` — resolves against the
packaged `lib/<abi>/libxiaoman_android.so` (`arm64-v8a` and `x86_64` by default). A missing library
is logged and reported on the host screen instead of crashing the service.

## Behavior the Kotlin side keeps

- **Threading.** Rust calls from its own runtime threads and never posts to a `Looper`. Anything
  that touches a window or the display hops to the main thread: `readRoots`, `editableFocus`,
  `globalBack`/`globalHome` (blocking hop), `captureScreen` and `dispatchStroke` (the Android call
  runs on main, while the *waiting* happens on the Rust thread — the main thread is never blocked).
  Reading node facts, children and performing actions run on the calling thread, like the old
  `Dispatchers.Default` tree walk.
- **Screenshots.** One `takeScreenshot` per 1050 ms, three attempts, retry on
  `ERROR_TAKE_SCREENSHOT_INTERVAL_TIME_SHORT`, `secure = true` on
  `ERROR_TAKE_SCREENSHOT_SECURE_WINDOW`. The bitmap is downscaled to a 1280 px long edge (never
  upscaled), sampled on the same 16×16 grid as `geometry::looks_all_black`, and an all-black frame
  is reported as `secure` — that is how Android 11–13 report a `FLAG_SECURE` window.
- **Gestures.** `dispatchStroke` takes `[x0, y0, x1, y1, …]`, builds the path on the main thread,
  dispatches with a `GestureResultCallback` on the main handler, and blocks the caller until
  completion or cancellation, so Rust sees one boolean per gesture.
- **Node handles.** A handle is an index into a `ConcurrentHashMap<Long, AccessibilityNodeInfo>`,
  starting at 1 so `0` stays `NodeHandle::NULL`. Every non-zero handle is released exactly once by
  Rust; `nodeRelease` removes it and returns the node to the platform pool on API < 33. The table is
  emptied on `onUnbind`/`onDestroy`.
- **Foreground app.** `TYPE_WINDOW_STATE_CHANGED` events are filtered exactly like the Kotlin app: a
  resolvable activity class wins; otherwise the package only replaces the current one when it really
  is the active window (not a toast or the IME).
- **Notification.** Same channel (`status`, IMPORTANCE_LOW), same title (小满正在工作), same Stop
  action (停止), `setOngoing`/`setOnlyAlertOnce`/`setSilent`, category `SERVICE`, immediate
  foreground behavior. The Stop action is a `PendingIntent.getService` with action
  `dev.xiaoman.app.AgentService.STOP`, handled in `onStartCommand`, which calls the instance native
  `nativeRequestStop`.
- **No overlay.** The Kotlin product shows a `TYPE_ACCESSIBILITY_OVERLAY` status pill. The contract
  in `bridge.rs` has no method for it (no show/update/hide, and `nativeRequestStop` is documented as
  "the overlay's Stop button"), so this shell has no overlay; the notification is the status and stop
  surface.

## Verified on the emulator, and what is still unverified

Built, installed and run on MuMu (Android 15, x86_64, API 35) on 2026-11-13. The APK is
`build/outputs/apk/debug/xiaoman-android-host-debug.apk` (24 MB, package `dev.xiaoman.host`,
`minSdk 30`, `targetSdk 35`, `lib/arm64-v8a` + `lib/x86_64` inside).

What the logcat lines prove (Rust logs under `XiaomanRust`, Kotlin under `Xiaoman`):

```
I Xiaoman     : libxiaoman_android.so loaded
I XiaomanRust : nativeOnServiceConnected: connected=true display=1920x1080 foreground-service=true
I XiaomanRust : nativeOnForeground: dev.xiaoman.host/dev.xiaoman.app.MainActivity
I XiaomanRust : nativeOnServiceUnbound: host dropped=true; the last foreground app is kept
I XiaomanRust : AgentService.nativeRequestStop: stop signal set
```

* `System.loadLibrary` resolves and the library is the real one, not the degraded path.
* All five `Java_dev_xiaoman_*` symbols (`llvm-nm -D`) are called by the JVM: the four
  `SERVICE_NATIVES` from `onServiceConnected`/`onUnbind`/window-state events, and
  `AgentService.nativeRequestStop` from the notification's Stop action.
* `connected=true display=1920x1080` is `jni_bridge` calling *back* into Java
  (`isConnected()Z`, `displayWidthPx()I`, `displayHeightPx()I`), so the `SERVICE_METHODS`
  descriptors match the Kotlin declarations field for field.
* `foreground-service=true` is `JniForegroundHost::from_context` resolving
  `dev.xiaoman.app.AgentService` with `FindClass` on the JVM-owned `onServiceConnected` thread.
* `dumpsys accessibility` while enabled shows
  `Bound services:{Service[label=小满, capabilities=161, eventTypes=TYPES_ALL_MASK]}` (161 =
  window content + gestures + screenshots) and `Enabled services:{{…XiaomanAccessibilityService}}`,
  `Crashed services:{}`; the host screen shows 无障碍服务=已开启, Rust 库=已加载.
* Enabling: `adb shell settings put secure enabled_accessibility_services
  dev.xiaoman.host/dev.xiaoman.hands.XiaomanAccessibilityService` and
  `settings put secure accessibility_enabled 1`; deleting the first key and setting the second to
  `0` unbinds cleanly and re-enabling binds again.

Still unverified on a device — the paths nobody has exercised yet, all of which mirror the shipped
Kotlin app:

* `captureScreen()` (`AccessibilityService.TakeScreenshotCallback`, the `FLAG_SECURE` black-frame
  heuristic) and `dispatchStroke()`'s `GestureResultCallback` wait.
* The node table: `readRoots`, `nodeFactsJson`, `nodeRelease` ownership and the
  `TYPE_WINDOW_STATE_CHANGED` filters against real apps.
* `JniForegroundHost`'s four static calls (`start`, `stop`, `isRunning`, `updateStatus`): the
  Kotlin statics resolve, but no Rust code drives them yet.

