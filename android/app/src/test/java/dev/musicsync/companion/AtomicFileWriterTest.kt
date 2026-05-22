package dev.musicsync.companion

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class AtomicFileWriterTest {

    @get:Rule val tmp = TemporaryFolder()

    @Test
    fun writes_full_payload_atomically() {
        val target = File(tmp.root, "deep/dir/song.mp3")
        val bytes = ByteArray(1024) { (it % 251).toByte() }
        AtomicFileWriter.writeStream(target, bytes.size.toLong(), reader(bytes))
        assertTrue(target.exists())
        assertArrayEquals(bytes, target.readBytes())
    }

    @Test
    fun replaces_existing_file() {
        val target = File(tmp.root, "x.mp3")
        target.writeBytes(byteArrayOf(0))
        val bytes = byteArrayOf(1, 2, 3, 4)
        AtomicFileWriter.writeStream(target, bytes.size.toLong(), reader(bytes))
        assertArrayEquals(bytes, target.readBytes())
    }

    @Test
    fun short_stream_leaves_no_temp_file_and_no_target() {
        val target = File(tmp.root, "short.mp3")
        val partial = byteArrayOf(1, 2, 3)
        var threw = false
        try {
            AtomicFileWriter.writeStream(target, 10L, reader(partial))
        } catch (_: Exception) {
            threw = true
        }
        assertTrue("should throw on short stream", threw)
        assertFalse("partial target should not be visible", target.exists())
        // No .tmp.* artifact left behind in the parent directory.
        val leftover = tmp.root.listFiles().orEmpty().filter { it.name.contains(".tmp.") }
        assertEquals("no leftover temp file", emptyList<File>(), leftover)
    }

    @Test
    fun write_text_round_trip() {
        val target = File(tmp.root, "p.m3u")
        AtomicFileWriter.writeText(target, "#EXTM3U\nfoo/bar.mp3\n")
        assertEquals("#EXTM3U\nfoo/bar.mp3\n", target.readText())
    }

    @Test
    fun cleanup_removes_temp_files() {
        File(tmp.root, "song.mp3.tmp.abc").apply { writeBytes(ByteArray(10)) }
        File(tmp.root, "song.mp3").apply { writeBytes(ByteArray(10)) }
        AtomicFileWriter.cleanupOrphanedTemps(tmp.root)
        assertTrue(File(tmp.root, "song.mp3").exists())
        assertFalse(File(tmp.root, "song.mp3.tmp.abc").exists())
    }

    private fun reader(bytes: ByteArray): AtomicFileWriter.ByteReader =
        object : AtomicFileWriter.ByteReader {
            var pos = 0
            override fun read(buf: ByteArray, off: Int, len: Int): Int {
                val avail = bytes.size - pos
                if (avail <= 0) return -1
                val n = minOf(len, avail)
                System.arraycopy(bytes, pos, buf, off, n)
                pos += n
                return n
            }
        }
}
