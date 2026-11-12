// Standalone Gradle project for the Android front-end: it packages
// libxiaoman_gpui.so into a NativeActivity host (`dev.gpui.mobile.GpuiInputActivity`,
// from gpui-mobile) so the five screens run natively in Rust.
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

rootProject.name = "XiaomanGpui"
include(":app")
