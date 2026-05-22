package dev.musicsync.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.assertFalse
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class ManifestBuilderTest {

    @get:Rule val tmp = TemporaryFolder()

    private fun makeFile(path: String, bytes: ByteArray = ByteArray(0)): File {
        val f = File(tmp.root, path)
        f.parentFile?.mkdirs()
        f.writeBytes(bytes)
        return f
    }

    @Test
    fun reports_relative_paths_under_root() {
        makeFile("Artist/Album/A.mp3", ByteArray(100))
        makeFile("Artist/Album/B.mp3", ByteArray(200))
        val (files, playlists) = ManifestBuilder.build(tmp.root)
        assertEquals(2, files.size)
        assertTrue(playlists.isEmpty())
        val paths = files.map { it.path }.sorted()
        assertEquals(listOf("Artist/Album/A.mp3", "Artist/Album/B.mp3"), paths)
        assertEquals(100L, files.first { it.path.endsWith("A.mp3") }.size)
    }

    @Test
    fun reports_playlists_separately_with_content() {
        makeFile("Music.m3u", "#EXTM3U\nArtist/Album/A.mp3\n".toByteArray())
        makeFile("Artist/Album/A.mp3", ByteArray(10))
        val (files, playlists) = ManifestBuilder.build(tmp.root)
        assertEquals(1, files.size)
        assertEquals(1, playlists.size)
        assertEquals("Music.m3u", playlists[0].name)
        assertEquals("#EXTM3U\nArtist/Album/A.mp3\n", playlists[0].content)
    }

    @Test
    fun skips_hidden_files() {
        makeFile(".trashed-thing.mp3", ByteArray(50))
        makeFile("regular.mp3", ByteArray(50))
        val (files, _) = ManifestBuilder.build(tmp.root)
        assertEquals(1, files.size)
        assertEquals("regular.mp3", files[0].path)
    }

    @Test
    fun empty_for_missing_root() {
        val nonexistent = File(tmp.root, "no-such-dir")
        val (files, playlists) = ManifestBuilder.build(nonexistent)
        assertTrue(files.isEmpty())
        assertTrue(playlists.isEmpty())
    }

    @Test
    fun path_traversal_rejected() {
        assertNull(validateRelativePath("../etc/passwd"))
        assertNull(validateRelativePath("/etc/passwd"))
        assertNull(validateRelativePath("foo/../bar.mp3"))
        assertNull(validateRelativePath(""))
        assertNull(validateRelativePath("a\\b.mp3"))
    }

    @Test
    fun path_normalisation_keeps_safe_paths() {
        assertEquals("Artist/Album/A.mp3", validateRelativePath("Artist/Album/A.mp3"))
        assertEquals("Artist/Album/A.mp3", validateRelativePath("./Artist/Album/A.mp3"))
        assertEquals("Artist/Album/A.mp3", validateRelativePath("Artist//Album///A.mp3"))
    }
}
