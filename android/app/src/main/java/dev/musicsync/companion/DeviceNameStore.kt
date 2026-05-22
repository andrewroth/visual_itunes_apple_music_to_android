package dev.musicsync.companion

import android.content.Context
import android.os.Build

/**
 * Persists a user-overridable device name. Used for the mDNS service
 * record AND inside HELLO_OK / PAIR_CHALLENGE so the desktop sees the same
 * name everywhere. Defaults to Build.MODEL (e.g. "Pixel 7") on first run.
 *
 * Plain SharedPreferences — not sensitive. Lives separately from the
 * encrypted TokenStore so a rename never touches the pairing token.
 */
class DeviceNameStore(context: Context) {

    private val prefs = context.getSharedPreferences("musicsync_meta", Context.MODE_PRIVATE)

    fun get(): String {
        return prefs.getString(KEY_NAME, null)
            ?: (Build.MODEL ?: "Android")
    }

    fun set(name: String) {
        val trimmed = name.trim().take(64)
        prefs.edit().putString(KEY_NAME, trimmed.ifBlank { Build.MODEL ?: "Android" }).apply()
    }

    fun resetToDefault() {
        prefs.edit().remove(KEY_NAME).apply()
    }

    companion object {
        private const val KEY_NAME = "device_name"
    }
}
