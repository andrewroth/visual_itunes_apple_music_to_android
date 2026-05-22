package dev.musicsync.companion

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import androidx.documentfile.provider.DocumentFile

/**
 * Persists the user-chosen music root as a Storage Access Framework
 * tree URI. The phone grants read+write to whatever directory the user
 * picks via Android's native folder picker, plus all subdirectories.
 *
 * On set(), we call takePersistableUriPermission so the grant survives
 * app restarts and device reboots. Without that, the URI is only valid
 * for the current process.
 *
 * The "display path" (e.g. /sdcard/Music) is a best-effort decoding for
 * showing in the UI and reporting in HELLO_OK; the actual I/O always
 * goes through the URI via DocumentFile.
 */
class MusicRootStore(context: Context) {

    private val ctx = context.applicationContext
    private val prefs = ctx.getSharedPreferences("musicsync_meta", Context.MODE_PRIVATE)

    /** The currently-stored tree URI, or null when nothing has been picked. */
    fun getUri(): Uri? {
        val s = prefs.getString(KEY_ROOT_URI, null) ?: return null
        val uri = Uri.parse(s)
        // Verify we still have permission. The system can revoke if the
        // user clears app data on the documents provider, etc.
        val still = ctx.contentResolver.persistedUriPermissions.any { it.uri == uri }
        return if (still) uri else null
    }

    /** The chosen folder as a DocumentFile, or null if no root is set. */
    fun getRoot(): DocumentFile? {
        val uri = getUri() ?: return null
        return DocumentFile.fromTreeUri(ctx, uri)
    }

    /**
     * Best-effort decoded path for display purposes, e.g. "/sdcard/Music".
     * Returns a placeholder string when nothing's set or the URI scheme
     * isn't externalstorage-flavoured.
     */
    fun getDisplayPath(): String {
        val uri = getUri() ?: return "(not chosen)"
        return decodeTreeUriToPath(uri) ?: uri.toString()
    }

    /**
     * Save a new root. The caller must have obtained the URI from
     * ACTION_OPEN_DOCUMENT_TREE with the `Intent.FLAG_GRANT_*` flags so we
     * can persist permission. Returns null on success, or an error string.
     */
    fun trySet(uri: Uri, grantFlags: Int): String? {
        try {
            ctx.contentResolver.takePersistableUriPermission(
                uri,
                grantFlags and (
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or
                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION
                ),
            )
        } catch (e: Exception) {
            return "Couldn't persist folder permission: ${e.message}"
        }
        val df = DocumentFile.fromTreeUri(ctx, uri)
        if (df == null || !df.isDirectory) {
            return "Picked item isn't a folder."
        }
        prefs.edit().putString(KEY_ROOT_URI, uri.toString()).apply()
        return null
    }

    fun clear() {
        getUri()?.let { uri ->
            try {
                ctx.contentResolver.releasePersistableUriPermission(
                    uri,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or
                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                )
            } catch (_: Exception) { /* OK */ }
        }
        prefs.edit().remove(KEY_ROOT_URI).apply()
    }

    companion object {
        private const val KEY_ROOT_URI = "music_root_uri"

        /**
         * Decode a SAF tree URI into a best-effort filesystem path string.
         * Used purely for display: I/O always uses the URI via DocumentFile.
         *
         * Returns null when the URI is from a provider we can't decode
         * (Google Drive, downloads provider, etc.).
         */
        fun decodeTreeUriToPath(uri: Uri): String? {
            val docId = try {
                DocumentsContract.getTreeDocumentId(uri)
            } catch (_: Exception) { return null }
            val parts = docId.split(":", limit = 2)
            if (parts.size != 2) return null
            val volume = parts[0]
            val rest = parts[1]
            val base = if (volume == "primary") "/sdcard" else "/storage/$volume"
            val joined = if (rest.isEmpty()) base else "$base/$rest"
            return joined.trimEnd('/').ifBlank { null }
        }
    }
}
