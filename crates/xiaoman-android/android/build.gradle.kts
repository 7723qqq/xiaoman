// The Android host of `crates/xiaoman-android`: the JVM shell around `libxiaoman_android.so`.
//
// Kotlin keeps the Android objects (the AccessibilityService, the foreground service, the
// launcher) and Rust keeps every decision. The two halves are joined by the method and native
// tables in `crates/xiaoman-android/src/bridge.rs`; this module adds nothing that is not in them.
import java.io.File
import java.util.Locale

plugins {
    // Pinned to the versions this machine's Gradle cache already holds; the repository's root
    // catalog asks for AGP 8.7.3 / Kotlin 2.1.0, which would have to be downloaded.
    id("com.android.application") version "8.9.2"
    id("org.jetbrains.kotlin.android") version "2.0.21"
    id("org.jetbrains.kotlin.plugin.compose") version "2.0.21"
}

/** `crates/xiaoman-android`, the Rust crate this APK hosts. */
val rustCrateDir: File = file("..")

/** Where `cargoNdk` puts the shared libraries; `jniLibs` reads it as an extra source directory. */
val rustJniLibsDir = layout.buildDirectory.dir("rustJniLibs")

/** Cargo's target directory. The workspace `target/` by default, so command line builds share it. */
val cargoTargetDir: File =
    providers.gradleProperty("xiaoman.cargoTargetDir").orNull?.let { file(it) } ?: file("../../../target")

/** `minSdk`: the API every NDK wrapper and the packaged library target. */
val androidApi = 30

/** ABIs to build. The emulator is x86_64; every current phone is arm64-v8a. */
val rustAbis: List<String> = providers.gradleProperty("xiaoman.abis").orElse("arm64-v8a,x86_64").get()
    .split(',').map { it.trim() }.filter { it.isNotEmpty() }

val abiTriples = mapOf(
    "arm64-v8a" to "aarch64-linux-android",
    "armeabi-v7a" to "armv7-linux-androideabi",
    "x86_64" to "x86_64-linux-android",
)

/** NDK clang prefix per ABI; the triple differs from clang's name for 32-bit ARM. */
val abiClangPrefix = mapOf(
    "arm64-v8a" to "aarch64-linux-android",
    "armeabi-v7a" to "armv7a-linux-androideabi",
    "x86_64" to "x86_64-linux-android",
)

/** Runs a command in the crate directory; returns the merged output and the exit code. */
fun runCommand(command: List<String>, env: Map<String, String> = emptyMap()): Pair<Int, String> {
    val process = ProcessBuilder(command)
        .directory(rustCrateDir)
        .redirectErrorStream(true)
        .apply { environment().putAll(env) }
        .start()
    val output = process.inputStream.bufferedReader().use { it.readText() }
    return process.waitFor() to output
}

/** `xiaoman.ndkDir`, then `ANDROID_NDK_HOME`, then `ANDROID_NDK_ROOT`; null when none is set. */
fun ndkDir(): File? = listOfNotNull(
    providers.gradleProperty("xiaoman.ndkDir").orNull,
    System.getenv("ANDROID_NDK_HOME"),
    System.getenv("ANDROID_NDK_ROOT"),
).map { file(it) }.firstOrNull { it.isDirectory }

/**
 * Cargo's linker environment for one ABI, taken from the NDK when one is configured.
 *
 * The wrapper is `<ndk>/toolchains/llvm/prebuilt/<host>/bin/<prefix><api>-clang` — the API level in
 * its name is the `minSdk` the library is linked against.
 */
fun linkerEnv(abi: String): Map<String, String> {
    val ndk = ndkDir() ?: return emptyMap()
    val os = System.getProperty("os.name").lowercase(Locale.ROOT)
    val host = when {
        os.startsWith("windows") -> "windows-x86_64"
        os.startsWith("mac") -> "darwin-x86_64"
        else -> "linux-x86_64"
    }
    val clangName = abiClangPrefix.getValue(abi) + androidApi + "-clang" +
        if (host.startsWith("windows")) ".cmd" else ""
    val clang = File(ndk, "toolchains/llvm/prebuilt/$host/bin/$clangName")
    if (!clang.isFile) return emptyMap()
    val triple = abiTriples.getValue(abi)
    val envTriple = triple.uppercase(Locale.ROOT).replace('-', '_')
    // `cc-rs` (ring and the other C dependencies) wants the archiver too; the NDK ships llvm-ar
    // and llvm-ranlib in the same bin directory. Both spellings are set because cargo passes
    // `CC_<triple>` with hyphens while cc-rs may look the target up with underscores.
    val ar = File(clang.parentFile, "llvm-ar").absolutePath
    val ranlib = File(clang.parentFile, "llvm-ranlib").absolutePath
    return mapOf(
        "CARGO_TARGET_${envTriple}_LINKER" to clang.absolutePath,
        "CC_$triple" to clang.absolutePath,
        "AR_$triple" to ar,
        "RANLIB_$triple" to ranlib,
        "AR_$envTriple" to ar,
        "RANLIB_$envTriple" to ranlib,
    )
}

android {
    // The R and BuildConfig package. The classes keep their contract packages
    // (`dev.xiaoman.hands`, `dev.xiaoman.app`); the application id only has to differ from the
    // Kotlin product so both can be installed side by side.
    namespace = "dev.xiaoman.host"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.xiaoman.host"
        minSdk = androidApi
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        // Only the ABIs `cargoNdk` actually wrote; an ABI filter also keeps the APK's supported
        // ABI list honest on a device that asks.
        ndk { abiFilters += rustAbis }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    sourceSets.getByName("main") {
        // `System.loadLibrary("xiaoman_android")` resolves through this directory once `cargoNdk`
        // has written build/rustJniLibs/<abi>/libxiaoman_android.so.
        jniLibs.srcDir(rustJniLibsDir.get().asFile)
        // `skills/examples` ships as `assets/skills/...`; HostApplication extracts it on first run.
        assets.srcDir(layout.buildDirectory.dir("generated/bundledSkills"))
    }

    packaging {
        resources.excludes += setOf("META-INF/{AL2.0,LGPL2.1}")
    }
}

kotlin { jvmToolchain(17) }

/** `skills/examples` -> `assets/skills`, so `HostApplication` can extract the bundled skills. */
val syncBundledSkills = tasks.register<Sync>("syncBundledSkills") {
    from(file("../../../skills/examples"))
    into(layout.buildDirectory.dir("generated/bundledSkills/skills"))
}

tasks.named("preBuild").configure { dependsOn(syncBundledSkills) }

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    implementation(platform("androidx.compose:compose-bom:2024.12.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
}

/**
 * Builds `libxiaoman_android.so` for every ABI in [rustAbis] and copies it into `jniLibs`.
 *
 * `crates/xiaoman-android/Cargo.toml` declares `crate-type = ["cdylib"]`, so a plain
 * `cargo build --release --target <triple>` produces the shared library; the linker comes from
 * [linkerEnv] (`CARGO_TARGET_<TRIPLE>_LINKER`). No `cargo-ndk` needed.
 */
val cargoNdk = tasks.register("cargoNdk") {
    group = "rust"
    description = "Builds libxiaoman_android.so with cargo and copies it into jniLibs (ABIs: ${rustAbis.joinToString()})."
    onlyIf("xiaoman.rust.skip is not true") { providers.gradleProperty("xiaoman.rust.skip").orNull != "true" }
    inputs.dir(rustCrateDir.resolve("src"))
    inputs.files(rustCrateDir.resolve("Cargo.toml"), rustCrateDir.resolve("../../Cargo.lock"))
    inputs.property("abis", rustAbis)
    outputs.dir(rustJniLibsDir)

    doLast {
        val destination = rustJniLibsDir.get().asFile.apply { mkdirs() }
        val manifest = rustCrateDir.resolve("Cargo.toml")
        for (abi in rustAbis) {
            val triple = abiTriples[abi]
                ?: throw GradleException("cargoNdk: unknown ABI '$abi' in xiaoman.abis (known: ${abiTriples.keys.joinToString()}).")
            val env = linkerEnv(abi)
            if (env.isEmpty()) {
                throw GradleException(
                    "cargoNdk: no NDK linker for $abi. Set ANDROID_NDK_HOME (or ANDROID_NDK_ROOT), " +
                        "or pass -Pxiaoman.ndkDir=<ndk>.",
                )
            }
            val command = listOf(
                "cargo", "build", "--release", "--target", triple,
                "--manifest-path", manifest.absolutePath,
                "--target-dir", cargoTargetDir.absolutePath,
            )
            logger.lifecycle("cargoNdk: ${command.joinToString(" ")}")
            val (exit, output) = runCommand(command, env)
            if (exit != 0) {
                throw GradleException("cargoNdk: cargo failed for $abi (exit $exit):\n$output")
            }
            val built = cargoTargetDir.resolve("$triple/release/libxiaoman_android.so")
            if (!built.isFile) {
                throw GradleException(
                    "cargoNdk: libxiaoman_android.so was not produced for $abi (looked in $built).\n" +
                        "crates/xiaoman-android/Cargo.toml must declare\n" +
                        "[lib]\ncrate-type = [\"cdylib\"]\n$output",
                )
            }
            val library = File(File(destination, abi).apply { mkdirs() }, "libxiaoman_android.so")
            built.copyTo(library, overwrite = true)
            logger.lifecycle("cargoNdk: $abi -> $library")
        }
    }
}

// The library has to exist before the manifest merge and packaging look at jniLibs.
tasks.named("preBuild") { dependsOn(cargoNdk) }
