// The GPUI spike's APK: one NativeActivity whose whole UI is Rust.
import java.io.File
import java.util.Base64
import java.util.Locale
import java.util.Properties

plugins {
    id("com.android.application") version "8.9.2"
    id("org.jetbrains.kotlin.android") version "2.0.21"
}

/** `crates/xiaoman-gpui`. */
val crateDir: File = file("../..")

/** Where `cargoBuild` writes the shared library; `jniLibs` reads it as an extra source dir. */
val rustJniLibsDir = layout.buildDirectory.dir("rustJniLibs")

/** The crate's own target directory, shared with `cargo check` for faster rebuilds. */
val cargoTargetDir: File =
    providers.gradleProperty("xiaoman.cargoTargetDir").orNull?.let { file(it) } ?: crateDir.resolve("target")

/** NDK: `xiaoman.ndkDir`, then `ANDROID_NDK_HOME`, then `ANDROID_NDK_ROOT`. */
fun ndkDir(): File? = listOf(
    providers.gradleProperty("xiaoman.ndkDir").orNull,
    System.getenv("ANDROID_NDK_HOME"),
    System.getenv("ANDROID_NDK_ROOT"),
).filterNotNull().map { file(it) }.firstOrNull { it.isDirectory }

/** The `aarch64-linux-android30-clang` wrapper (`.cmd` on Windows). */
fun linkerEnv(): Map<String, String> {
    val ndk = ndkDir() ?: return emptyMap()
    val os = System.getProperty("os.name").lowercase(Locale.ROOT)
    val host = when {
        os.startsWith("windows") -> "windows-x86_64"
        os.startsWith("mac") -> "darwin-x86_64"
        else -> "linux-x86_64"
    }
    val suffix = if (host.startsWith("windows")) ".cmd" else ""
    val clang = File(ndk, "toolchains/llvm/prebuilt/$host/bin/aarch64-linux-android30-clang$suffix")
    if (!clang.isFile) return emptyMap()
    val ar = File(clang.parentFile, "llvm-ar").absolutePath
    return mapOf(
        "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER" to clang.absolutePath,
        "CC_aarch64-linux-android" to clang.absolutePath,
        "AR_aarch64-linux-android" to ar,
        "AR_aarch64_linux_android" to ar,
        "RANLIB_aarch64_linux_android" to File(clang.parentFile, "llvm-ranlib").absolutePath,
        "RUST_FONTCONFIG_DLOPEN" to "on",
    )
}

/**
 * Release signing credentials. Environment variables win over the local properties files, and
 * nothing here is committed: `~/.xiaoman-android/xiaoman-signing.properties` and
 * `local.properties` are both outside version control (or ignored).
 *
 * Variables: `XIAOMAN_SIGNING_STORE_FILE` / `_STORE_PASSWORD` / `_KEY_ALIAS` / `_KEY_PASSWORD`,
 * or `XIAOMAN_SIGNING_KEYSTORE_BASE64` (a base64 keystore, decoded into `build/signing/`).
 */
fun releaseSigning(): Map<String, String> {
    val values = mutableMapOf<String, String>()
    listOf(
        File(System.getProperty("user.home"), ".xiaoman-android/xiaoman-signing.properties"),
        rootProject.projectDir.resolve("local.properties"),
    ).filter { it.isFile }.forEach { file ->
        val properties = Properties()
        file.inputStream().use { properties.load(it) }
        for ((key, value) in properties) {
            val name = key as? String ?: continue
            if (name.startsWith("xiaoman.signing.")) {
                values[name.removePrefix("xiaoman.signing.")] = value as String
            }
        }
    }
    System.getenv("XIAOMAN_SIGNING_KEYSTORE_BASE64")?.takeIf { it.isNotBlank() }?.let { encoded ->
        val target = layout.buildDirectory.file("signing/xiaoman-release.keystore").get().asFile
        target.parentFile.mkdirs()
        target.writeBytes(Base64.getDecoder().decode(encoded))
        values["storeFile"] = target.absolutePath
    }
    mapOf(
        "storeFile" to "XIAOMAN_SIGNING_STORE_FILE",
        "storePassword" to "XIAOMAN_SIGNING_STORE_PASSWORD",
        "keyAlias" to "XIAOMAN_SIGNING_KEY_ALIAS",
        "keyPassword" to "XIAOMAN_SIGNING_KEY_PASSWORD",
    ).forEach { (key, variable) -> System.getenv(variable)?.let { values[key] = it } }
    return values
}

android {
    namespace = "dev.xiaoman.app"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.xiaoman.app"
        minSdk = 30
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        ndk { abiFilters += "arm64-v8a" }
        manifestPlaceholders["nativeLibraryName"] = "xiaoman_gpui"
    }

    signingConfigs {
        val signing = releaseSigning()
        val keys = listOf("storeFile", "storePassword", "keyAlias", "keyPassword")
        if (keys.all { signing[it]?.isNotBlank() == true }) {
            create("xiaomanRelease") {
                storeFile = file(signing.getValue("storeFile"))
                storePassword = signing.getValue("storePassword")
                keyAlias = signing.getValue("keyAlias")
                keyPassword = signing.getValue("keyPassword")
            }
        } else {
            logger.warn("signing: no release credentials, assembleRelease will produce an unsigned APK")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfigs.findByName("xiaomanRelease")?.let { signingConfig = it }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets.getByName("main") {
        jniLibs.srcDir(rustJniLibsDir.get().asFile)
        // `skills/examples` ships as `assets/skills/...`; HostApplication extracts it on first run.
        assets.srcDir(layout.buildDirectory.dir("generated/bundledSkills"))
    }
    sourceSets.getByName("debug") {
        // The `xiaoman` CLI's bridge and the smoke receiver are exported `adb` entry points, so they
        // live in the debug source set and never reach a release APK.
        kotlin.srcDir("src/debug/kotlin")
    }
}

kotlin { jvmToolchain(17) }

dependencies {
    // NotificationCompat / ContextCompat in the copied Kotlin services.
    implementation("androidx.core:core-ktx:1.13.1")
}

/** `skills/examples` -> `assets/skills`, extracted by HostApplication on first run. */
val syncBundledSkills = tasks.register<Sync>("syncBundledSkills") {
    from(crateDir.resolve("../../skills/examples"))
    into(layout.buildDirectory.dir("generated/bundledSkills/skills"))
}

tasks.named("preBuild").configure { dependsOn(syncBundledSkills) }

/** `cargo build --release --target aarch64-linux-android`, then copy the `.so` into jniLibs. */
val cargoBuild = tasks.register("cargoBuild") {
    group = "rust"
    description = "Builds libxiaoman_gpui.so for arm64-v8a."
    inputs.dir(crateDir.resolve("src"))
    // Every crate the APK links: a change in any of them must rebuild the `.so`.
    listOf(
        "xiaoman-android", "xiaoman-app", "xiaoman-api", "xiaoman-sentinel", "xiaoman-llm",
        "xiaoman-memory", "xiaoman-skills", "xiaoman-agent", "xiaoman-mcp",
    ).forEach { crate ->
        inputs.dir(crateDir.resolve("../$crate/src"))
        inputs.files(crateDir.resolve("../$crate/Cargo.toml"))
    }
    inputs.files(crateDir.resolve("../../Cargo.toml"), crateDir.resolve("../../Cargo.lock"))
    // The vendored mobile platform crate is part of the build too.
    inputs.dir(crateDir.resolve("vendor/gpui-mobile/src"))
    inputs.files(crateDir.resolve("vendor/gpui-mobile/Cargo.toml"))
    inputs.files(crateDir.resolve("Cargo.toml"), crateDir.resolve("Cargo.lock"))
    outputs.dir(rustJniLibsDir)

    doLast {
        val linker = linkerEnv()
        if (linker.isEmpty()) {
            throw GradleException("cargoBuild: no NDK. Set ANDROID_NDK_HOME or pass -Pxiaoman.ndkDir=<ndk>.")
        }
        // The crate is a workspace member, so `[profile.release]` cannot live in its Cargo.toml;
        // keep the APK's size settings through cargo's environment overrides.
        val env = linker.toMutableMap().apply {
            put("CARGO_PROFILE_RELEASE_OPT_LEVEL", "z")
            put("CARGO_PROFILE_RELEASE_LTO", "thin")
            put("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "1")
            put("CARGO_PROFILE_RELEASE_STRIP", "true")
        }
        val destination = rustJniLibsDir.get().asFile.resolve("arm64-v8a").apply { mkdirs() }
        val command = listOf(
            "cargo", "build", "--release", "--target", "aarch64-linux-android",
            "--manifest-path", crateDir.resolve("Cargo.toml").absolutePath,
            "--target-dir", cargoTargetDir.absolutePath,
        )
        logger.lifecycle("cargoBuild: ${command.joinToString(" ")}")
        val process = ProcessBuilder(command)
            .directory(crateDir)
            .redirectErrorStream(true)
            .apply { environment().putAll(env) }
            .start()
        val output = process.inputStream.bufferedReader().use { it.readText() }
        if (process.waitFor() != 0) {
            throw GradleException("cargoBuild failed:\n$output")
        }
        val built = cargoTargetDir.resolve("aarch64-linux-android/release/libxiaoman_gpui.so")
        if (!built.isFile) throw GradleException("cargoBuild: missing ${built.absolutePath}")
        built.copyTo(destination.resolve("libxiaoman_gpui.so"), overwrite = true)
        logger.lifecycle("cargoBuild: wrote ${destination.resolve("libxiaoman_gpui.so")}")
    }
}

tasks.named("preBuild").configure { dependsOn(cargoBuild) }
