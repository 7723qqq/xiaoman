// Standalone build for the Android host of `crates/xiaoman-android`.
//
// This directory is its own Gradle build: it is not included by the repository's root
// `settings.gradle.kts`, and it does not include `:app` or `:hands`. The root project of this build
// *is* the application module, so `build.gradle.kts` next to this file is the app.
pluginManagement {
    repositories {
        google()
        maven("https://repo1.maven.org/maven2/")
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        maven("https://repo1.maven.org/maven2/")
    }
}

rootProject.name = "xiaoman-android-host"
