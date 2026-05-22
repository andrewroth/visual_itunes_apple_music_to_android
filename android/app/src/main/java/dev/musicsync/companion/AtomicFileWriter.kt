package dev.musicsync.companion

import android.content.ContentResolver
import androidx.documentfile.provider.DocumentFile
import java.io.FileOutputStream
import java.io.OutputStream
import java.nio.charset.StandardCharsets
import java.util.UUID

/**
 * Atomic file writes via the SAF DocumentFile API. The pattern:
 *  1. Stream bytes to a temp DocumentFile named "<target>.tmp.<uuid>"
 *  2. fsync if we can reach the underlying FileDescriptor
 *  3. Rename the temp to the final name (DocumentFile.renameTo)
 *
 * An interrupted upload leaves only the .tmp file behind, which the
 * manifest builder ignores (the final name doesn't exist, so the manifest
 * accurately reports the file as missing and the next sync retries it).
 */
object AtomicFileWriter {

    fun interface ByteReader {
        fun read(buf: ByteArray, off: Int, len: Int): Int
    }

    /** Write [size] bytes from [reader] into [relPath] under [root].
     *  Creates parent directories as needed. */
    fun writeStream(
        root: DocumentFile,
        relPath: String,
        size: Long,
        reader: ByteReader,
        resolver: ContentResolver,
    ) {
        val parts = relPath.split('/').filter { it.isNotEmpty() }
        require(parts.isNotEmpty()) { "empty path" }
        val targetName = parts.last()
        val parent = parts.dropLast(1).fold(root) { dir, segment ->
            findOrCreateDir(dir, segment)
        }

        val tmpName = "$targetName.tmp.${UUID.randomUUID()}"
        val tmp = parent.createFile("application/octet-stream", tmpName)
            ?: throw IllegalStateException("could not create temp file $tmpName")
        try {
            resolver.openOutputStream(tmp.uri, "w")?.use { out ->
                copyExactly(reader, out, size)
                if (out is FileOutputStream) {
                    try { out.fd.sync() } catch (_: Exception) { /* best effort */ }
                }
            } ?: throw IllegalStateException("openOutputStream returned null")

            // If a previous version of the target exists, delete it first
            // (renameTo on an occupied name fails on most providers).
            parent.findFile(targetName)?.delete()
            if (!tmp.renameTo(targetName)) {
                throw IllegalStateException("rename ${tmp.name} -> $targetName failed")
            }
        } catch (e: Exception) {
            tmp.delete()
            throw e
        }
    }

    /** Atomic text write — used for playlists. */
    fun writeText(
        root: DocumentFile,
        relPath: String,
        text: String,
        resolver: ContentResolver,
    ) {
        val bytes = text.toByteArray(StandardCharsets.UTF_8)
        writeStream(
            root,
            relPath,
            bytes.size.toLong(),
            object : ByteReader {
                var pos = 0
                override fun read(buf: ByteArray, off: Int, len: Int): Int {
                    val avail = bytes.size - pos
                    if (avail <= 0) return -1
                    val n = minOf(len, avail)
                    System.arraycopy(bytes, pos, buf, off, n)
                    pos += n
                    return n
                }
            },
            resolver,
        )
    }

    /** Look up an immediate child directory by name; create if missing. */
    private fun findOrCreateDir(parent: DocumentFile, name: String): DocumentFile {
        val existing = parent.findFile(name)
        if (existing != null && existing.isDirectory) return existing
        if (existing != null) {
            // A file already exists with this name. Can't make a directory.
            throw IllegalStateException("conflicting file at ${parent.name}/$name")
        }
        return parent.createDirectory(name)
            ?: throw IllegalStateException("could not create directory $name under ${parent.name}")
    }

    /** Pull exactly `size` bytes from the reader and write them out. */
    private fun copyExactly(reader: ByteReader, out: OutputStream, size: Long) {
        val buf = ByteArray(64 * 1024)
        var remaining = size
        while (remaining > 0) {
            val want = minOf(buf.size.toLong(), remaining).toInt()
            val n = reader.read(buf, 0, want)
            if (n <= 0) throw IllegalStateException("stream ended with $remaining bytes remaining")
            out.write(buf, 0, n)
            remaining -= n
        }
    }
}

/** Path-traversal guard. Returns a sanitised relative path or null if invalid. */
internal fun validateRelativePath(input: String): String? {
    if (input.isBlank()) return null
    if (input.startsWith('/') || input.contains('\\')) return null
    val cleaned = input
        .removePrefix("./")
        .trim('/')
        .replace(Regex("/+"), "/")
    if (cleaned.isEmpty()) return null
    // Reject ".." as a whole path SEGMENT, not as a substring — filenames
    // are allowed to contain consecutive dots ("Dismantle.Repair..m4a").
    for (seg in cleaned.split('/')) {
        if (seg == ".." || seg == ".") return null
    }
    return cleaned
}
