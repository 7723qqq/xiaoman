// The GPUI spike's APK: one NativeActivity whose whole UI is Rust.
import java.io.File
import java.util.Locale

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

android {
    namespace = "dev.xiaoman.host"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.xiaoman.host"
        minSdk = 30
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        ndk { abiFilters += "arm64-v8a" }
        manifestPlaceholders["nativeLibraryName"] = "xiaoman_gpui"
    }

    buildTypes {
        release { isMinifyEnabled = false }
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
