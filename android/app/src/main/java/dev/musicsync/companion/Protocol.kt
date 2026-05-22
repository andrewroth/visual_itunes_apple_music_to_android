package dev.musicsync.companion

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonClassDiscriminator
import kotlinx.serialization.ExperimentalSerializationApi

/**
 * Wire protocol types — see PROTOCOL.md at the project root for the full
 * specification. These mirror the Rust enums in core/src/protocol.rs;
 * any change here must be made in lock-step on the Rust side.
 */

const val PROTOCOL_VERSION = 1
const val DEFAULT_PORT = 7800

@OptIn(ExperimentalSerializationApi::class)
@JsonClassDiscriminator("kind")
@Serializable
sealed class ClientMessage {
    @Serializable
    @SerialName("HELLO")
    data class Hello(
        val token: String,
        val protocol_version: Int,
        val desktop_user: String = "",
        val desktop_host: String = "",
    ) : ClientMessage()

    @Serializable
    @SerialName("PAIR_REQUEST")
    data class PairRequest(
        val protocol_version: Int,
        val desktop_user: String = "",
        val desktop_host: String = "",
    ) : ClientMessage()

    @Serializable
    @SerialName("PAIR_CONFIRM")
    data object PairConfirm : ClientMessage()

    @Serializable
    @SerialName("PAIR_CANCEL")
    data object PairCancel : ClientMessage()

    @Serializable
    @SerialName("MANIFEST_REQUEST")
    data object ManifestRequest : ClientMessage()

    @Serializable
    @SerialName("FILE_PUT")
    data class FilePut(val path: String, val size: Long) : ClientMessage()

    @Serializable
    @SerialName("PLAYLIST_PUT")
    data class PlaylistPut(val name: String, val content: String) : ClientMessage()

    @Serializable
    @SerialName("FILE_DELETE")
    data class FileDelete(val path: String) : ClientMessage()

    @Serializable
    @SerialName("PROGRESS")
    data class Progress(
        val message: String,
        val fraction: Float? = null,
    ) : ClientMessage()

    @Serializable
    @SerialName("BYE")
    data object Bye : ClientMessage()
}

@OptIn(ExperimentalSerializationApi::class)
@JsonClassDiscriminator("kind")
@Serializable
sealed class ServerMessage {
    @Serializable
    @SerialName("HELLO_OK")
    data class HelloOk(
        val device_id: String,
        val device_name: String,
        val music_root: String,
        val protocol_version: Int,
    ) : ServerMessage()

    @Serializable
    @SerialName("PAIR_CHALLENGE")
    data class PairChallenge(
        val code: String,
        val device_id: String,
        val device_name: String,
    ) : ServerMessage()

    @Serializable
    @SerialName("PAIR_OK")
    data class PairOk(
        val token: String,
        val device_id: String,
        val device_name: String,
        val music_root: String,
    ) : ServerMessage()

    /**
     * Pushed unsolicited over an open session when the user renames the
     * phone. The desktop updates its display label without re-pairing or
     * re-scanning — identity is the immutable [device_id].
     */
    @Serializable
    @SerialName("DEVICE_RENAMED")
    data class DeviceRenamed(
        val device_id: String,
        val device_name: String,
    ) : ServerMessage()

    @Serializable
    @SerialName("PAIR_CANCELLED")
    data class PairCancelled(val reason: String) : ServerMessage()

    @Serializable
    @SerialName("MANIFEST")
    data class Manifest(
        val files: List<ManifestFile>,
        val playlists: List<ManifestPlaylist>,
    ) : ServerMessage()

    @Serializable
    @SerialName("FILE_OK")
    data class FileOk(val path: String) : ServerMessage()

    @Serializable
    @SerialName("FILE_ERR")
    data class FileErr(val path: String, val message: String) : ServerMessage()

    @Serializable
    @SerialName("PLAYLIST_OK")
    data class PlaylistOk(val name: String) : ServerMessage()

    @Serializable
    @SerialName("PLAYLIST_ERR")
    data class PlaylistErr(val name: String, val message: String) : ServerMessage()

    @Serializable
    @SerialName("FILE_DELETE_OK")
    data class FileDeleteOk(val path: String) : ServerMessage()

    @Serializable
    @SerialName("FILE_DELETE_ERR")
    data class FileDeleteErr(val path: String, val message: String) : ServerMessage()

    @Serializable
    @SerialName("PROGRESS")
    data class Progress(val message: String, val fraction: Float? = null) : ServerMessage()

    @Serializable
    @SerialName("BYE")
    data object Bye : ServerMessage()

    @Serializable
    @SerialName("ERROR")
    data class Error(val message: String) : ServerMessage()
}

@Serializable
data class ManifestFile(
    val path: String,
    val size: Long,
    val mtime: Long,
)

@Serializable
data class ManifestPlaylist(
    val name: String,
    val mtime: Long,
    val content: String,
)
