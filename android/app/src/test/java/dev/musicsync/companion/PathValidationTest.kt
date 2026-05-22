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

    @Test
    fun consecutive_dots_inside_segment_are_allowed() {
        // Real filename from a user's library — was being false-rejected
        // by a substring check for "..". Legal: ".." is only dangerous
        // as a whole path segment.
        assertEquals(
            "Anberlin/Cities/11 Dismantle.Repair..m4a",
            validateRelativePath("Anberlin/Cities/11 Dismantle.Repair..m4a"),
        )
        assertEquals("foo..bar.mp3", validateRelativePath("foo..bar.mp3"))
    }
}
