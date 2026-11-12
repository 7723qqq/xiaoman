package dev.xiaoman.app

import android.Manifest
import android.content.ComponentName
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.provider.Settings
import androidx.core.content.ContextCompat
import dev.xiaoman.hands.XiaomanAccessibilityService

/** True when the user has this app's accessibility service in the enabled list. */
internal fun accessibilityEnabled(context: Context): Boolean {
    val expected = ComponentName(context, XiaomanAccessibilityService::class.java)
    val enabled = Settings.Secure.getString(
        context.contentResolver,
        Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
    ) ?: return false
    return enabled.split(':')
        .mapNotNull { ComponentName.unflattenFromString(it) }
        .any { it == expected }
}

/** POST_NOTIFICATIONS, granted by default before Android 13. */
internal fun notificationsAllowed(context: Context): Boolean =
    Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
        ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
        PackageManager.PERMISSION_GRANTED
