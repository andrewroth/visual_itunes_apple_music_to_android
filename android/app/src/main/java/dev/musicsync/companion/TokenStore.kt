package dev.musicsync.companion

import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.security.SecureRandom

/**
 * One known desktop's record. Lives in the Manage Approvals list with
 * either `approved = true` (HELLO with this token succeeds) or
 * `approved = false` (HELLO is silently rejected without prompting the
 * user again — the user can remove the entry to re-prompt).
 *
 * `user` and `host` come from PAIR_REQUEST / HELLO; "unknown" for legacy
 * single-token entries migrated from the previous schema.
 */
@Serializable
data class PairedDesktop(
    val token: String,
    val user: String,
    val host: String,
    val pairedAtMs: Long,
    /** Default true for backwards compatibility — entries from the
     *  previous schema were all approved by definition. */
    val approved: Boolean = true,
)

/**
 * Encrypted-storage list of all paired desktops. Multi-pair model:
 *  - `addPairing` appends a new entry with a fresh random token
 *  - `verify(supplied)` returns true if any entry's token matches
 *  - `removeByToken` / `clearAll` for forget operations
 *
 * Storage migrates from the previous single-token schema on first read:
 * a legacy `KEY_TOKEN` entry is converted into a one-element list and
 * the legacy key is deleted, so existing pairings survive the upgrade.
 */
class TokenStore(context: Context) {

    private val ctx = context.applicationContext
    private val prefs: SharedPreferences by lazy {
        val master = MasterKey.Builder(ctx)
            .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
            .build()
        val sp = EncryptedSharedPreferences.create(
            ctx,
            "musicsync_secrets",
            master,
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
        )
        // One-shot migration from the legacy single-token schema.
        val legacy = sp.getString(LEGACY_KEY_TOKEN, null)
        if (legacy != null && sp.getString(KEY_DESKTOPS, null) == null) {
            val migrated = listOf(
                PairedDesktop(
                    token = legacy,
                    user = "unknown",
                    host = "unknown",
                    pairedAtMs = System.currentTimeMillis(),
                ),
            )
            sp.edit()
                .putString(KEY_DESKTOPS, json.encodeToString(migrated))
                .remove(LEGACY_KEY_TOKEN)
                .apply()
        }
        sp
    }

    fun list(): List<PairedDesktop> {
        val s = prefs.getString(KEY_DESKTOPS, null) ?: return emptyList()
        return try {
            json.decodeFromString(s)
        } catch (_: Exception) {
            emptyList()
        }
    }

    /** True if at least one APPROVED desktop is in the list. */
    fun hasAny(): Boolean = list().any { it.approved }

    /** Returns true iff `supplied` matches an APPROVED token. */
    fun verify(supplied: String): Boolean {
        for (entry in list()) {
            if (!entry.approved) continue
            if (constantTimeEquals(entry.token, supplied)) return true
        }
        return false
    }

    /** Returns true iff `supplied` matches an explicitly-DENIED token. */
    fun isDenied(supplied: String): Boolean {
        for (entry in list()) {
            if (entry.approved) continue
            if (constantTimeEquals(entry.token, supplied)) return true
        }
        return false
    }

    private fun constantTimeEquals(a: String, b: String): Boolean {
        if (a.length != b.length) return false
        var diff = 0
        for (i in a.indices) diff = diff or (a[i].code xor b[i].code)
        return diff == 0
    }

    /** Generate + persist a new pairing entry. Returns the token to issue
     *  to the desktop in PAIR_OK. */
    fun addPairing(user: String, host: String): String {
        val token = generate()
        val updated = list() + PairedDesktop(
            token = token,
            user = user.ifBlank { "unknown" },
            host = host.ifBlank { "unknown" },
            pairedAtMs = System.currentTimeMillis(),
        )
        prefs.edit()
            .putString(KEY_DESKTOPS, json.encodeToString(updated))
            .apply()
        return token
    }

    /** Add the supplied (already-known-to-the-desktop) token to the
     *  approval list as APPROVED. Used when the phone user approves a
     *  HELLO from an otherwise-unrecognised token. */
    fun acceptExistingToken(token: String, user: String, host: String) {
        upsert(token, user, host, approved = true)
    }

    /** Record an explicit DENIAL so future HELLOs from this token are
     *  auto-rejected without prompting again. */
    fun denyToken(token: String, user: String, host: String) {
        upsert(token, user, host, approved = false)
    }

    private fun upsert(token: String, user: String, host: String, approved: Boolean) {
        val now = System.currentTimeMillis()
        val current = list().toMutableList()
        val idx = current.indexOfFirst { it.token == token }
        val entry = PairedDesktop(
            token = token,
            user = user.ifBlank { "unknown" },
            host = host.ifBlank { "unknown" },
            pairedAtMs = if (idx >= 0) current[idx].pairedAtMs else now,
            approved = approved,
        )
        if (idx >= 0) current[idx] = entry else current.add(entry)
        prefs.edit()
            .putString(KEY_DESKTOPS, json.encodeToString(current))
            .apply()
    }

    fun removeByToken(token: String) {
        val updated = list().filter { it.token != token }
        prefs.edit()
            .putString(KEY_DESKTOPS, json.encodeToString(updated))
            .apply()
    }

    fun clearAll() {
        prefs.edit().remove(KEY_DESKTOPS).apply()
    }

    private fun generate(): String {
        val bytes = ByteArray(18)
        SecureRandom().nextBytes(bytes)
        val alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"
        val sb = StringBuilder(bytes.size * 2)
        for ((i, b) in bytes.withIndex()) {
            sb.append(alphabet[(b.toInt() and 0xFF) % alphabet.length])
            if (i % 4 == 3 && i < bytes.lastIndex) sb.append('-')
        }
        return sb.toString()
    }

    companion object {
        private const val KEY_DESKTOPS = "paired_desktops"
        private const val LEGACY_KEY_TOKEN = "device_token"
        private val json = Json { ignoreUnknownKeys = true }
    }
}
