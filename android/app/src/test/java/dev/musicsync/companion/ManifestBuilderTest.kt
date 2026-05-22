package dev.musicsync.companion

import android.content.ContentResolver
import android.database.Cursor
import android.net.Uri
import android.provider.DocumentsContract
import androidx.documentfile.provider.DocumentFile
import io.mockk.every
import io.mockk.mockk
import io.mockk.mockkStatic
import io.mockk.unmockkAll
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.ByteArrayInputStream

class ManifestBuilderTest {

    private lateinit var resolver: ContentResolver
    private lateinit var rootDoc: DocumentFile
    private lateinit var treeUri: Uri

    private data class Row(val docId: String, val name: String, val mime: String, val mtime: Long, val size: Long)

    private val tree = mutableMapOf<String, List<Row>>()
    private val playlistBody = mutableMapOf<String, String>()
    private val uriParent = mutableMapOf<Uri, String>()
    private val uriDoc = mutableMapOf<Uri, String>()

    @Before
    fun setup() {
        treeUri = mockk()
        rootDoc = mockk()
        resolver = mockk()
        every { rootDoc.uri } returns treeUri
        every { rootDoc.isDirectory } returns true

        mockkStatic(DocumentsContract::class)
        every { DocumentsContract.getTreeDocumentId(treeUri) } returns "ROOT"
        every { DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, any()) } answers {
            val parentId = secondArg<String>()
            mockk<Uri>().also { uriParent[it] = parentId }
        }
        every { DocumentsContract.buildDocumentUriUsingTree(treeUri, any()) } answers {
            val docId = secondArg<String>()
            mockk<Uri>().also { uriDoc[it] = docId }
        }

        every { resolver.query(any(), any(), null, null, null) } answers {
            val parentId = uriParent.getValue(firstArg())
            fakeCursor(tree[parentId].orEmpty())
        }
        every { resolver.openInputStream(any()) } answers {
            ByteArrayInputStream(playlistBody.getValue(uriDoc.getValue(firstArg())).toByteArray())
        }
    }

    @After fun tearDown() { unmockkAll() }

    @Test
    fun reports_relative_paths_under_root() {
        tree["ROOT"] = listOf(Row("artist", "Artist", DocumentsContract.Document.MIME_TYPE_DIR, 0, 0))
        tree["artist"] = listOf(Row("album", "Album", DocumentsContract.Document.MIME_TYPE_DIR, 0, 0))
        tree["album"] = listOf(
            Row("a", "A.mp3", "audio/mpeg", 1000, 100),
            Row("b", "B.mp3", "audio/mpeg", 2000, 200),
        )
        val (files, playlists) = ManifestBuilder.build(rootDoc, resolver)
        assertEquals(2, files.size)
        assertTrue(playlists.isEmpty())
        assertEquals(
            listOf("Artist/Album/A.mp3", "Artist/Album/B.mp3"),
            files.map { it.path }.sorted(),
        )
        assertEquals(100L, files.first { it.path.endsWith("A.mp3") }.size)
    }

    @Test
    fun reports_playlists_separately_with_content() {
        playlistBody["m3u-id"] = "#EXTM3U\nArtist/Album/A.mp3\n"
        tree["ROOT"] = listOf(
            Row("m3u-id", "Music.m3u", "audio/x-mpegurl", 0, 0),
            Row("artist", "Artist", DocumentsContract.Document.MIME_TYPE_DIR, 0, 0),
        )
        tree["artist"] = listOf(Row("album", "Album", DocumentsContract.Document.MIME_TYPE_DIR, 0, 0))
        tree["album"] = listOf(Row("a", "A.mp3", "audio/mpeg", 0, 10))
        val (files, playlists) = ManifestBuilder.build(rootDoc, resolver)
        assertEquals(1, files.size)
        assertEquals(1, playlists.size)
        assertEquals("Music.m3u", playlists[0].name)
        assertEquals("#EXTM3U\nArtist/Album/A.mp3\n", playlists[0].content)
    }

    @Test
    fun skips_hidden_files() {
        tree["ROOT"] = listOf(
            Row("h", ".trashed-thing.mp3", "audio/mpeg", 0, 50),
            Row("r", "regular.mp3", "audio/mpeg", 0, 50),
        )
        val (files, _) = ManifestBuilder.build(rootDoc, resolver)
        assertEquals(1, files.size)
        assertEquals("regular.mp3", files[0].path)
    }

    @Test
    fun empty_for_missing_root() {
        every { rootDoc.isDirectory } returns false
        val (files, playlists) = ManifestBuilder.build(rootDoc, resolver)
        assertTrue(files.isEmpty())
        assertTrue(playlists.isEmpty())
    }

    private fun fakeCursor(rows: List<Row>): Cursor {
        val c = mockk<Cursor>()
        var pos = -1
        every { c.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DOCUMENT_ID) } returns 0
        every { c.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DISPLAY_NAME) } returns 1
        every { c.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_MIME_TYPE) } returns 2
        every { c.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_LAST_MODIFIED) } returns 3
        every { c.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_SIZE) } returns 4
        every { c.moveToNext() } answers { pos++; pos < rows.size }
        every { c.getString(0) } answers { rows[pos].docId }
        every { c.getString(1) } answers { rows[pos].name }
        every { c.getString(2) } answers { rows[pos].mime }
        every { c.isNull(any()) } returns false
        every { c.getLong(3) } answers { rows[pos].mtime }
        every { c.getLong(4) } answers { rows[pos].size }
        every { c.close() } returns Unit
        return c
    }
}
