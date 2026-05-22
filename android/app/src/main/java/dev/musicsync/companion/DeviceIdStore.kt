package dev.musicsync.companion

import android.content.Context
import java.util.UUID

/**
 * Stable per-install identifier reported to paired desktops so they can
 * recognise this phone across renames, IP changes, and TXT-record edits.
 *
 * Generated once on first read and persisted to plain SharedPreferences
 * (not sensitive). Forgetting all pairings does NOT rotate this id —
 * a phone is the same phone whether or not any desktop currently trusts
 * it. Rotation would only confuse desktops still trying the old token.
 */
class DeviceIdStore(context: Context) {

    private val prefs = context.getSharedPreferences("musicsync_meta", Context.MODE_PRIVATE)

    fun get(): String {
        val existing = prefs.getString(KEY_ID, null)
        if (!existing.isNullOrEmpty()) return existing
        val fresh = UUID.randomUUID().toString()
        prefs.edit().putString(KEY_ID, fresh).apply()
        return fresh
    }

    companion object {
        private const val KEY_ID = "device_id"
    }
}
