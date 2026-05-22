package dev.musicsync.companion

import android.content.ContentResolver
import android.net.Uri
import android.provider.DocumentsContract
import androidx.documentfile.provider.DocumentFile
import java.io.BufferedReader
import java.io.InputStreamReader
import java.nio.charset.StandardCharsets

/**
 * Walks the music folder (a SAF tree URI) and builds a manifest matching
 * the wire protocol.
 *
 * Performance note: we deliberately do NOT use DocumentFile.listFiles().
 * That API returns DocumentFile wrappers that each issue a fresh content-
 * provider IPC for every metadata accessor (name, mtime, size, mime).
 * With ~10 metadata accesses per child × thousands of files, the IPC
 * overhead alone takes 60-90 seconds for a typical music library.
 *
 * Instead we query DocumentsContract.buildChildDocumentsUriUsingTree
 * directly with a column projection — one query per directory, all the
 * metadata returned together in a single cursor. ~100× faster.
 */
object ManifestBuilder {

    data class ScanProgress(
        val filesSoFar: Int,
        val topLevelDone: Int,
        val topLevelTotal: Int,
    )

    private val COLUMNS = arrayOf(
        DocumentsContract.Document.COLUMN_DOCUMENT_ID,
        DocumentsContract.Document.COLUMN_DISPLAY_NAME,
        DocumentsContract.Document.COLUMN_MIME_TYPE,
        DocumentsContract.Document.COLUMN_LAST_MODIFIED,
        DocumentsContract.Document.COLUMN_SIZE,
    )

    fun build(
        musicRoot: DocumentFile?,
        resolver: ContentResolver,
        onProgress: (ScanProgress) -> Unit = {},
    ): Pair<List<ManifestFile>, List<ManifestPlaylist>> {
        val files = mutableListOf<ManifestFile>()
        val playlists = mutableListOf<ManifestPlaylist>()
        if (musicRoot == null || !musicRoot.isDirectory) {
            onProgress(ScanProgress(0, 0, 0))
            return files to playlists
        }

        val treeUri = musicRoot.uri
        val rootDocId = DocumentsContract.getTreeDocumentId(treeUri)

        val rootChildren = listChildren(resolver, treeUri, rootDocId)
        val topDirs = rootChildren.filter { it.isDir && !it.name.startsWith(".") }
        val total = topDirs.size
        onProgress(ScanProgress(filesSoFar = 0, topLevelDone = 0, topLevelTotal = total))

        for (child in rootChildren) {
            if (!child.isDir && !child.name.startsWith(".")) {
                processFile(child, child.name, files, playlists, resolver, treeUri)
            }
        }

        for ((i, dir) in topDirs.withIndex()) {
            walk(
                treeUri = treeUri,
                docId = dir.docId,
                relPath = dir.name,
                files = files,
                playlists = playlists,
                resolver = resolver,
            ) { fileCount -> onProgress(ScanProgress(fileCount, i, total)) }
            onProgress(ScanProgress(files.size, i + 1, total))
        }

        onProgress(ScanProgress(files.size, total, total))
        return files to playlists
    }

    private fun walk(
        treeUri: Uri,
        docId: String,
        relPath: String,
        files: MutableList<ManifestFile>,
        playlists: MutableList<ManifestPlaylist>,
        resolver: ContentResolver,
        onFileProgress: (Int) -> Unit,
    ) {
        val children = listChildren(resolver, treeUri, docId)
        for (child in children) {
            val name = child.name
            if (name.startsWith(".")) continue
            val childRel = "$relPath/$name"
            if (child.isDir) {
                walk(treeUri, child.docId, childRel, files, playlists, resolver, onFileProgress)
            } else {
                processFile(child, childRel, files, playlists, resolver, treeUri)
                if (files.size % 200 == 0) onFileProgress(files.size)
            }
        }
    }

    private data class ChildRow(
        val docId: String,
        val name: String,
        val mime: String,
        val mtime: Long,
        val size: Long,
    ) {
        val isDir: Boolean get() = mime == DocumentsContract.Document.MIME_TYPE_DIR
    }

    private fun listChildren(
        resolver: ContentResolver,
        treeUri: Uri,
        parentDocId: String,
    ): List<ChildRow> {
        val childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, parentDocId)
        val out = ArrayList<ChildRow>()
        resolver.query(childrenUri, COLUMNS, null, null, null)?.use { cursor ->
            val idCol = cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DOCUMENT_ID)
            val nameCol = cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DISPLAY_NAME)
            val mimeCol = cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_MIME_TYPE)
            val mtimeCol = cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_LAST_MODIFIED)
            val sizeCol = cursor.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_SIZE)
            while (cursor.moveToNext()) {
                out.add(ChildRow(
                    docId = cursor.getString(idCol),
                    name = cursor.getString(nameCol) ?: "",
                    mime = cursor.getString(mimeCol) ?: "",
                    mtime = if (cursor.isNull(mtimeCol)) 0L else cursor.getLong(mtimeCol),
                    size = if (cursor.isNull(sizeCol)) 0L else cursor.getLong(sizeCol),
                ))
            }
        }
        return out
    }

    private fun processFile(
        child: ChildRow,
        relPath: String,
        files: MutableList<ManifestFile>,
        playlists: MutableList<ManifestPlaylist>,
        resolver: ContentResolver,
        treeUri: Uri,
    ) {
        val name = child.name
        if (name.startsWith(".")) return
        val mtimeSec = child.mtime / 1000L
        if (name.endsWith(".m3u", ignoreCase = true)) {
            val docUri = DocumentsContract.buildDocumentUriUsingTree(treeUri, child.docId)
            val content = try {
                resolver.openInputStream(docUri)?.use { input ->
                    BufferedReader(InputStreamReader(input, StandardCharsets.UTF_8))
                        .readText()
                } ?: ""
            } catch (_: Exception) { "" }
            playlists.add(ManifestPlaylist(name = relPath, mtime = mtimeSec, content = content))
        } else {
            files.add(ManifestFile(path = relPath, size = child.size, mtime = mtimeSec))
        }
    }
}
