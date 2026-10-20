package dev.musicsync.companion

import org.junit.Assert.assertEquals
import org.junit.Test

class EmptyFolderPrunerTest {
    /** Fake folder tree: name → number of entries inside it. */
    private class Tree(vararg counts: Pair<String, Int>) {
        val entries = counts.toMap().toMutableMap()
        val deleted = mutableListOf<String>()
        var undeletable: String? = null

        fun prune(vararg parents: String): Int = pruneEmptyParents(
            parents.toList(),
            isEmpty = { entries.getValue(it) == 0 },
            delete = { name ->
                if (name == undeletable) {
                    false
                } else {
                    deleted += name
                    // Removing a folder empties one slot in its parent.
                    val i = parents.indexOf(name)
                    if (i > 0) entries[parents[i - 1]] = entries.getValue(parents[i - 1]) - 1
                    true
                }
            },
        )
    }

    @Test
    fun removesEmptyAlbumThenEmptyArtist() {
        val tree = Tree("Artist" to 1, "Album" to 0)
        assertEquals(2, tree.prune("Artist", "Album"))
        assertEquals(listOf("Album", "Artist"), tree.deleted)
    }

    @Test
    fun stopsAtFirstFolderThatStillHasContent() {
        // Artist has another album left.
        val tree = Tree("Artist" to 2, "Album" to 0)
        assertEquals(1, tree.prune("Artist", "Album"))
        assertEquals(listOf("Album"), tree.deleted)
    }

    @Test
    fun leavesFolderWithOtherTracksAlone() {
        val tree = Tree("Artist" to 1, "Album" to 3)
        assertEquals(0, tree.prune("Artist", "Album"))
        assertEquals(emptyList<String>(), tree.deleted)
    }

    @Test
    fun stopsWhenADeleteFails() {
        val tree = Tree("Artist" to 1, "Album" to 0)
        tree.undeletable = "Album"
        assertEquals(0, tree.prune("Artist", "Album"))
        assertEquals(emptyList<String>(), tree.deleted)
    }

    @Test
    fun fileDirectlyInMusicRootPrunesNothing() {
        assertEquals(0, Tree().prune())
    }
}
