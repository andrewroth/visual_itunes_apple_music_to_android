package dev.musicsync.companion

import android.content.ContentResolver
import android.net.Uri
import androidx.documentfile.provider.DocumentFile
import io.mockk.every
import io.mockk.mockk
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.io.FileOutputStream

class AtomicFileWriterTest {

    @get:Rule val tmp = TemporaryFolder()

    private lateinit var resolver: ContentResolver
    private val backing = mutableMapOf<Uri, File>()

    @Before
    fun setup() {
        resolver = mockk()
        every { resolver.openOutputStream(any(), "w") } answers {
            FileOutputStream(backing.getValue(firstArg()))
        }
    }

    @Test
    fun writes_full_payload_atomically() {
        val root = mockDir(tmp.root)
        val bytes = ByteArray(1024) { (it % 251).toByte() }
        AtomicFileWriter.writeStream(root, "deep/dir/song.mp3", bytes.size.toLong(), reader(bytes), resolver)
        val target = File(tmp.root, "deep/dir/song.mp3")
        assertTrue(target.exists())
        assertArrayEquals(bytes, target.readBytes())
    }

    @Test
    fun replaces_existing_file() {
        val root = mockDir(tmp.root)
        File(tmp.root, "x.mp3").writeBytes(byteArrayOf(0))
        val bytes = byteArrayOf(1, 2, 3, 4)
        AtomicFileWriter.writeStream(root, "x.mp3", bytes.size.toLong(), reader(bytes), resolver)
        assertArrayEquals(bytes, File(tmp.root, "x.mp3").readBytes())
    }

    @Test
    fun short_stream_leaves_no_temp_file_and_no_target() {
        val root = mockDir(tmp.root)
        val partial = byteArrayOf(1, 2, 3)
        var threw = false
        try {
            AtomicFileWriter.writeStream(root, "short.mp3", 10L, reader(partial), resolver)
        } catch (_: Exception) {
            threw = true
        }
        assertTrue("should throw on short stream", threw)
        assertFalse("partial target should not be visible", File(tmp.root, "short.mp3").exists())
        val leftover = tmp.root.listFiles().orEmpty().filter { it.name.contains(".tmp.") }
        assertEquals("no leftover temp file", emptyList<File>(), leftover)
    }

    @Test
    fun write_text_round_trip() {
        val root = mockDir(tmp.root)
        AtomicFileWriter.writeText(root, "p.m3u", "#EXTM3U\nfoo/bar.mp3\n", resolver)
        assertEquals("#EXTM3U\nfoo/bar.mp3\n", File(tmp.root, "p.m3u").readText())
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

    // Wires a mock DocumentFile to a real directory on disk so the writer's
    // findFile/createFile/createDirectory/renameTo/delete calls land on the
    // real filesystem and we can assert against File.exists()/readBytes().
    private fun mockDir(real: File): DocumentFile {
        real.mkdirs()
        val df = mockk<DocumentFile>()
        every { df.isDirectory } returns true
        every { df.name } returns real.name
        every { df.findFile(any()) } answers {
            val child = File(real, firstArg())
            if (!child.exists()) null
            else if (child.isDirectory) mockDir(child) else mockFile(child)
        }
        every { df.createDirectory(any()) } answers {
            val child = File(real, firstArg()).apply { mkdirs() }
            mockDir(child)
        }
        every { df.createFile(any(), any()) } answers {
            val child = File(real, secondArg<String>()).apply { createNewFile() }
            mockFile(child)
        }
        return df
    }

    private fun mockFile(real: File): DocumentFile {
        val df = mockk<DocumentFile>()
        val uri = mockk<Uri>()
        backing[uri] = real
        every { df.uri } returns uri
        every { df.name } returns real.name
        every { df.isDirectory } returns false
        every { df.delete() } answers { real.delete() }
        every { df.renameTo(any()) } answers {
            val dest = File(real.parentFile, firstArg())
            real.renameTo(dest).also { ok -> if (ok) backing[uri] = dest }
        }
        return df
    }
}
