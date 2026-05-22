package dev.musicsync.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PathValidationTest {

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
