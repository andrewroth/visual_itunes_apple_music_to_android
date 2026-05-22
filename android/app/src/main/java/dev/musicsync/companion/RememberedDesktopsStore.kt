package dev.musicsync.companion

import android.content.Context

/**
 * Tracks IP addresses of desktops that have successfully completed a
 * HELLO against this phone. On the next phone boot / Wi-Fi change the
 * companion proactively pings each remembered IP on UDP 7798 with a
 * "MUSICSYNC_HERE" packet — a direct unicast announcement that works
 * on networks where mDNS / broadcast probes get filtered.
 *
 * Not secret, so this lives in plain SharedPreferences (no encryption
 * needed). Capped at 10 entries with most-recently-seen kept; older
 * ones get evicted to keep the announcement work bounded.
 *
 * Storage format is a single newline-separated string. Trivial enough
 * that a JSON dependency would be overkill here.
 */
class RememberedDesktopsStore(context: Context) {

    private val prefs = context.applicationContext
        .getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun list(): List<String> {
        val raw = prefs.getString(KEY, "") ?: ""
        if (raw.isEmpty()) return emptyList()
        return raw.split('\n').map { it.trim() }.filter { it.isNotEmpty() }
    }

    /**
     * Insert [ip] at the head of the list (most-recently-seen first).
     * Dedups if already present; trims tail to [MAX_ENTRIES]. No-op if
     * [ip] is empty or looks like loopback.
     */
    fun remember(ip: String) {
        val cleaned = ip.trim()
        if (cleaned.isEmpty()) return
        if (cleaned.startsWith("127.") || cleaned == "::1") return
        val current = list().toMutableList()
        current.remove(cleaned)
        current.add(0, cleaned)
        while (current.size > MAX_ENTRIES) current.removeAt(current.size - 1)
        prefs.edit().putString(KEY, current.joinToString("\n")).apply()
    }

    fun clear() {
        prefs.edit().remove(KEY).apply()
    }

    companion object {
        private const val PREFS = "musicsync_remembered_desktops"
        private const val KEY = "ips"
        private const val MAX_ENTRIES = 10
    }
}
