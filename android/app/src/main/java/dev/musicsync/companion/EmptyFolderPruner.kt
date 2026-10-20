package dev.musicsync.companion

/**
 * After a file is deleted, removes the folders it leaves empty (e.g. the
 * album folder, then the artist folder) so deleted music doesn't leave a
 * tree of empty directories behind.
 *
 * [parents] are the folders between the music root and the deleted file,
 * outermost first; the music root itself must NOT be in the list, so it
 * can never be removed. Walks from the innermost folder outwards and stops
 * at the first folder that still has something in it or can't be deleted.
 *
 * Generic over the folder type so it can be unit-tested without SAF.
 *
 * @return how many folders were removed.
 */
internal fun <T> pruneEmptyParents(
    parents: List<T>,
    isEmpty: (T) -> Boolean,
    delete: (T) -> Boolean,
): Int {
    var removed = 0
    for (folder in parents.asReversed()) {
        if (!isEmpty(folder) || !delete(folder)) break
        removed++
    }
    return removed
}
