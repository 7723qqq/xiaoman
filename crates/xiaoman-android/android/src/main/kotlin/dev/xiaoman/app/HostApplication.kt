package dev.xiaoman.app

import android.app.Application
import android.util.Log
import dev.xiaoman.hands.NativeLibrary
import java.io.File

/**
 * Opens the one Rust session of this process before any activity or receiver needs it.
 *
 * The data directory is the app's external files directory, so `adb` can push skills and pull
 * traces at the same paths the CLI uses:
 * `/sdcard/Android/data/dev.xiaoman.host/files/{settings.json,memory,skills,traces}`.
 * The bundled example skills are copied out of `assets/skills` on first run (and after an update),
 * then searched before the user's own folder.
 */
class HostApplication : Application() {

    override fun onCreate() {
        super.onCreate()
        AlarmReminders.init(this)
        Notifications.createChannels(this)
        if (!NativeLibrary.loaded) {
            Log.w(TAG, "host: libxiaoman_android.so is not loaded; the session stays closed")
            return
        }
        val dataDir = (getExternalFilesDir(null) ?: filesDir).apply { mkdirs() }
        val bundled = extractBundledSkills()
        val opened = RustSession.open(dataDir.absolutePath, bundled?.absolutePath)
        Log.i(TAG, "host: session opened=$opened data=${dataDir.absolutePath} skills=${bundled?.absolutePath ?: "-"}")
    }

    /** `assets/skills/<name>/...` -> `filesDir/bundled-skills/<name>/...`, refreshed per install. */
    private fun extractBundledSkills(): File? {
        val names = runCatching { assets.list(ASSET_DIR).orEmpty().toList() }.getOrDefault(emptyList())
        if (names.isEmpty()) return null
        val target = File(filesDir, BUNDLED_DIR)
        val marker = File(target, MARKER)
        val version = runCatching {
            packageManager.getPackageInfo(packageName, 0).lastUpdateTime.toString()
        }.getOrDefault("1")
        if (marker.isFile && marker.readText() == version) return target

        target.deleteRecursively()
        target.mkdirs()
        for (name in names) copyAsset("$ASSET_DIR/$name", File(target, name))
        marker.writeText(version)
        Log.i(TAG, "host: copied ${names.size} bundled skills to ${target.absolutePath}")
        return target
    }

    private fun copyAsset(path: String, destination: File) {
        val children = runCatching { assets.list(path).orEmpty() }.getOrDefault(emptyArray())
        if (children.isEmpty()) {
            destination.parentFile?.mkdirs()
            assets.open(path).use { input -> destination.outputStream().use { input.copyTo(it) } }
        } else {
            destination.mkdirs()
            for (child in children) copyAsset("$path/$child", File(destination, child))
        }
    }

    private companion object {
        const val TAG = "Xiaoman"
        const val ASSET_DIR = "skills"
        const val BUNDLED_DIR = "bundled-skills"
        const val MARKER = ".version"
    }
}
