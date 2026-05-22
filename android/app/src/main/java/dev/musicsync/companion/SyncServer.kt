package dev.musicsync.companion

import android.content.ContentResolver
import android.os.Build
import androidx.documentfile.provider.DocumentFile
import io.ktor.server.application.install
import io.ktor.server.cio.CIO
import io.ktor.server.engine.embeddedServer
import io.ktor.server.routing.routing
import io.ktor.server.websocket.WebSockets
import io.ktor.server.websocket.webSocket
import io.ktor.websocket.CloseReason
import io.ktor.websocket.Frame
import io.ktor.websocket.close
import io.ktor.websocket.readBytes
import io.ktor.websocket.readText
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.Json
import java.util.concurrent.atomic.AtomicReference

/**
 * The Ktor WebSocket server. Hosts one endpoint (`/`) that speaks the
 * protocol defined in PROTOCOL.md. The server is single-session — there is
 * no expectation of multiple concurrent desktops trying to sync at once,
 * but if it happens each connection is independent and authenticates
 * independently.
 *
 * State lives in [Config]: where the music folder is and how to verify
 * tokens. The server is started/stopped from [SyncService].
 */
class SyncServer(private val config: Config) {

    data class Config(
        /** Read fresh each time so changes to the music root take effect
         *  on the next manifest/upload without restarting the server.
         *  Null when the user hasn't chosen a folder yet — all I/O
         *  operations return an error in that state. */
        val musicRoot: () -> DocumentFile?,
        /** Best-effort path display for HELLO_OK and UI. */
        val musicRootDisplay: () -> String,
        /** ContentResolver needed by SAF I/O. */
        val contentResolver: ContentResolver,
        /** Read fresh each time so renames take effect without restarting
         *  the server. The mDNS advertiser is restarted separately. */
        val deviceName: () -> String,
        /** Stable UUID for this phone. Sent in HELLO_OK / PAIR_OK /
         *  PAIR_CHALLENGE / DEVICE_RENAMED so the desktop pins identity
         *  to a value the user can't change. */
        val deviceId: () -> String,
        val verifyToken: (String) -> Boolean,
        /** Generates a brand-new token for this pair attempt and stores
         *  it on the phone. Receives the desktop's announced user+host
         *  for the on-phone display + later management. Returns the token
         *  to send back in PAIR_OK. */
        val issuePairingToken: (desktopUser: String, desktopHost: String) -> String,
        /** Suspends to ask the phone user "Approve this desktop?" when a
         *  HELLO arrives with an unrecognised token. Receives the
         *  desktop's self-reported user+host for display. Returns true
         *  if the user tapped Approve. */
        val requestApprovalForHello: suspend (user: String, host: String) -> Boolean,
        /** Whitelist the given token verbatim (used after the user
         *  approves an unrecognised HELLO). */
        val acceptExistingToken: (token: String, user: String, host: String) -> Unit,
        val pairingManager: PairingManager,
        /** Fired when a desktop completes pairing. Reports our deviceName
         *  (could be extended to identify the peer once we capture that). */
        val onPairSuccess: (deviceName: String) -> Unit = {},
        /** Fired when we receive MANIFEST_REQUEST. UI shows 'Scanning music…'. */
        val onScanStarted: () -> Unit = {},
        /** Fired periodically during the manifest walk. Reports running
         *  file count + how many top-level subdirs are done (for a
         *  determinate progress bar). */
        val onScanProgress: (filesSoFar: Int, topLevelDone: Int, topLevelTotal: Int) -> Unit
            = { _, _, _ -> },
        /** Fired after ManifestBuilder.build() returns. UI shows the result. */
        val onScanComplete: (files: Int, playlists: Int) -> Unit = { _, _ -> },
        /** Fired the first time a session does a file transfer (FILE_PUT,
         *  FILE_DELETE, or PLAYLIST_PUT). UI uses this to disable
         *  destructive controls (Choose music folder) and show a Stop. */
        val onSyncStarted: () -> Unit = {},
        /** Fired when a transfer-bearing session ends. */
        val onSyncEnded: () -> Unit = {},
        /** Fired with the labels of currently-connected desktops every
         *  time a session starts or ends (or its identity becomes known
         *  after auth). Each label is "user@host" (or "(connecting)"
         *  during the brief pre-auth window). Empty list = nothing
         *  connected. UI uses this for both the chip color and to show
         *  "Desktop andrew@192.168.0.42 connected". */
        val onClientsChanged: (labels: List<String>) -> Unit = {},
        val onEvent: (String) -> Unit = {},
        val port: Int = DEFAULT_PORT,
    )

    private val json = Json { ignoreUnknownKeys = true; classDiscriminator = "kind" }
    private val server = AtomicReference<io.ktor.server.engine.ApplicationEngine?>(null)

    /**
     * Live websocket sessions with their friendly label. We need both
     * the session (for [closeAllSessions]) and the label (for the UI chip).
     */
    private data class ActiveClient(
        val session: io.ktor.server.websocket.DefaultWebSocketServerSession,
        @Volatile var label: String,
    )
    private val activeClients =
        java.util.concurrent.CopyOnWriteArrayList<ActiveClient>()

    private fun fireClientsChanged() {
        config.onClientsChanged(activeClients.map { it.label })
    }

    /**
     * Push a DEVICE_RENAMED notification to every connected desktop on
     * the current set of live sessions. Used by [SyncService.renameDevice]
     * so the desktop UI updates its "paired with X" banner instantly
     * without having to drop and re-establish the heartbeat connection.
     */
    fun broadcastRename(deviceId: String, newName: String) {
        if (deviceId.isEmpty()) return
        val snapshot = activeClients.toList()
        val msg = ServerMessage.DeviceRenamed(device_id = deviceId, device_name = newName)
        val text = json.encodeToString(ServerMessage.serializer(), msg)
        for (c in snapshot) {
            try {
                kotlinx.coroutines.runBlocking {
                    c.session.outgoing.send(Frame.Text(text))
                }
            } catch (_: Exception) {
                // Session is in the process of dying; nothing to do — the
                // desktop will pick up the new name on its next reconnect
                // via HELLO_OK anyway.
            }
        }
        config.onEvent("pushed rename to ${snapshot.size} desktop(s)")
    }

    /** Close every connected desktop. They will reconnect on their own
     *  next scan attempt and pick up the new state. */
    fun closeAllSessions(reason: String) {
        val snapshot = activeClients.toList()
        activeClients.clear()
        for (c in snapshot) {
            try {
                kotlinx.coroutines.runBlocking {
                    c.session.close(CloseReason(CloseReason.Codes.NORMAL, reason))
                }
            } catch (_: Exception) { /* already gone */ }
        }
        fireClientsChanged()
        config.onEvent("closed ${snapshot.size} active session(s): $reason")
    }

    fun start() {
        if (server.get() != null) return
        val engine = embeddedServer(CIO, port = config.port, host = "0.0.0.0") {
            install(WebSockets) {
                // Ping every 15s; drop sessions that don't pong in 30s.
                // Keeps NAT state warm for our persistent presence
                // connection and quickly detects vanished desktops.
                // Ktor 2.x uses millisecond fields, not Duration.
                pingPeriodMillis = 15_000L
                timeoutMillis = 30_000L
            }
            routing {
                webSocket("/") { handleSession(this) }
            }
        }
        engine.start(wait = false)
        server.set(engine)
        config.onEvent("listening on port ${config.port}")
    }

    fun stop() {
        server.getAndSet(null)?.stop(500, 1000)
        config.onEvent("server stopped")
    }

    /**
     * The state machine for one connection. Steps through HELLO, then
     * handles control messages in a loop. FILE_PUT control messages are
     * always followed by exactly one binary frame whose length matches
     * the announced size.
     */
    private suspend fun handleSession(
        session: io.ktor.server.websocket.DefaultWebSocketServerSession,
    ) {
        val client = ActiveClient(session, "(connecting)")
        activeClients.add(client)
        fireClientsChanged()
        config.onEvent("desktop connected")
        var didTransfer = false
        try {
            val firstText = nextText(session) ?: return
            val firstMsg = parseClient(firstText)

            // Branch on the first message: pairing or normal sync. Pairing
            // doesn't require a pre-existing token and goes through the
            // numeric-comparison handshake.
            if (firstMsg is ClientMessage.PairRequest) {
                // PAIR_REQUEST carries the desktop's identity verbatim.
                client.label = formatLabel(firstMsg.desktop_user, firstMsg.desktop_host)
                fireClientsChanged()
                handlePairingHandshake(session, firstMsg)
                return
            }

            val helloMsg = firstMsg as? ClientMessage.Hello
            if (helloMsg == null) {
                send(session, ServerMessage.Error("expected HELLO or PAIR_REQUEST"))
                session.close(CloseReason(CloseReason.Codes.VIOLATED_POLICY, "no HELLO"))
                return
            }
            if (helloMsg.protocol_version != PROTOCOL_VERSION) {
                send(session, ServerMessage.Error("protocol version mismatch"))
                session.close(CloseReason(CloseReason.Codes.VIOLATED_POLICY, "version"))
                return
            }
            if (!config.verifyToken(helloMsg.token)) {
                // Unknown token: ask the user. If approved we add the
                // token to the list; if denied we just close — no
                // persistent denial record, so a future probe from the
                // same desktop will prompt the user again.
                val approved = config.requestApprovalForHello(
                    helloMsg.desktop_user,
                    helloMsg.desktop_host,
                )
                if (!approved) {
                    send(session, ServerMessage.Error("bad token"))
                    session.close(CloseReason(CloseReason.Codes.VIOLATED_POLICY, "auth"))
                    return
                }
                config.acceptExistingToken(
                    helloMsg.token,
                    helloMsg.desktop_user,
                    helloMsg.desktop_host,
                )
            }
            // Auth passed (or the user just approved an unknown token);
            // adopt the desktop's identity for the UI chip.
            client.label = formatLabel(helloMsg.desktop_user, helloMsg.desktop_host)
            fireClientsChanged()
            send(
                session,
                ServerMessage.HelloOk(
                    device_id = config.deviceId(),
                    device_name = config.deviceName(),
                    music_root = config.musicRootDisplay().trimEnd('/') + "/",
                    protocol_version = PROTOCOL_VERSION,
                ),
            )

            while (true) {
                val text = nextText(session) ?: break
                val msg = parseClient(text)
                when (msg) {
                    is ClientMessage.ManifestRequest -> {
                        config.onScanStarted()
                        val root = config.musicRoot()
                        if (root == null) {
                            send(session, ServerMessage.Error("phone has no music folder set"))
                            config.onScanComplete(0, 0)
                            continue
                        }
                        val (files, playlists) = ManifestBuilder.build(
                            root,
                            config.contentResolver,
                            onProgress = { p ->
                                // Local UI signal (phone's own scan banner).
                                config.onScanProgress(p.filesSoFar, p.topLevelDone, p.topLevelTotal)
                                // Mirror to the connected desktop so it can
                                // render the same %. trySend is non-blocking
                                // and drops on backpressure — fine for a
                                // progress stream where the latest wins.
                                val frac = if (p.topLevelTotal > 0) {
                                    (p.topLevelDone.toFloat() / p.topLevelTotal.toFloat())
                                        .coerceIn(0f, 1f)
                                } else null
                                val msg = if (p.topLevelTotal > 0) {
                                    "Scanning phone: ${p.filesSoFar} files " +
                                        "(${p.topLevelDone}/${p.topLevelTotal} folders)"
                                } else {
                                    "Scanning phone: ${p.filesSoFar} files"
                                }
                                val progress = ServerMessage.Progress(message = msg, fraction = frac)
                                val text = json.encodeToString(ServerMessage.serializer(), progress)
                                session.outgoing.trySend(Frame.Text(text))
                            },
                        )
                        config.onScanComplete(files.size, playlists.size)
                        send(session, ServerMessage.Manifest(files, playlists))
                    }
                    is ClientMessage.FilePut -> {
                        if (!didTransfer) { didTransfer = true; config.onSyncStarted() }
                        handleFilePut(session, msg)
                    }
                    is ClientMessage.PlaylistPut -> {
                        if (!didTransfer) { didTransfer = true; config.onSyncStarted() }
                        handlePlaylistPut(session, msg)
                    }
                    is ClientMessage.FileDelete -> {
                        if (!didTransfer) { didTransfer = true; config.onSyncStarted() }
                        handleFileDelete(session, msg)
                    }
                    is ClientMessage.Bye -> {
                        send(session, ServerMessage.Bye)
                        session.close(CloseReason(CloseReason.Codes.NORMAL, "bye"))
                        return
                    }
                    is ClientMessage.Hello -> {
                        send(session, ServerMessage.Error("HELLO already sent"))
                    }
                    is ClientMessage.PairRequest,
                    is ClientMessage.PairConfirm,
                    is ClientMessage.PairCancel -> {
                        send(
                            session,
                            ServerMessage.Error("pairing messages only valid before HELLO"),
                        )
                    }
                }
            }
        } catch (e: Exception) {
            config.onEvent("session error: ${e.message}")
            try {
                send(session, ServerMessage.Error(e.message ?: "unknown error"))
            } catch (_: Exception) { /* socket already closed */ }
        } finally {
            activeClients.remove(client)
            fireClientsChanged()
            if (didTransfer) config.onSyncEnded()
            config.onEvent("desktop disconnected")
        }
    }

    private fun formatLabel(user: String, host: String): String {
        val u = user.ifBlank { "" }
        val h = host.ifBlank { "" }
        return when {
            u.isNotBlank() && h.isNotBlank() -> "$u@$h"
            u.isNotBlank() -> u
            h.isNotBlank() -> h
            else -> "(unknown)"
        }
    }

    /**
     * Bluetooth-style numeric comparison. We send the same 6-digit code to
     * the desktop and the phone UI. The user verifies the codes match and
     * taps Confirm on both. Only after BOTH confirmations does the phone
     * issue its persistent token.
     *
     * 60-second timeout, fail-closed on any cancel / unexpected message.
     */
    private suspend fun handlePairingHandshake(
        session: io.ktor.server.websocket.DefaultWebSocketServerSession,
        request: ClientMessage.PairRequest,
    ) {
        if (request.protocol_version != PROTOCOL_VERSION) {
            send(session, ServerMessage.Error("protocol version mismatch"))
            session.close(CloseReason(CloseReason.Codes.VIOLATED_POLICY, "version"))
            return
        }
        // Multi-pair model: every pair request is allowed; the user
        // confirms on the phone's dialog (which shows the desktop's
        // self-identification) to whitelist that specific machine.
        val code = PairingManager.generateCode()
        config.onEvent(
            "pairing started for ${request.desktop_user}@${request.desktop_host}, code=$code"
        )
        send(
            session,
            ServerMessage.PairChallenge(
                code = code,
                device_id = config.deviceId(),
                device_name = config.deviceName(),
            ),
        )

        // DefaultWebSocketServerSession is a CoroutineScope — `session.launch`
        // spawns children tied to the connection's lifetime. We must satisfy
        // BOTH halves (phone tap and client PAIR_CONFIRM) before issuing the
        // token; either side cancelling fails the pair.
        val phoneAck = CompletableDeferred<Boolean>()
        val phoneJob = session.launch {
            try {
                val ok = config.pairingManager.requestPair(
                    code = code,
                    deviceName = config.deviceName(),
                    desktopUser = request.desktop_user,
                    desktopHost = request.desktop_host,
                )
                phoneAck.complete(ok)
            } catch (e: Exception) {
                phoneAck.complete(false)
            }
        }

        val clientAck = CompletableDeferred<Boolean>()
        val readerJob = session.launch {
            try {
                while (true) {
                    val text = nextText(session) ?: break
                    when (parseClient(text)) {
                        is ClientMessage.PairConfirm -> {
                            clientAck.complete(true); break
                        }
                        is ClientMessage.PairCancel -> {
                            clientAck.complete(false); break
                        }
                        else -> { /* ignore stray msgs pre-pair-resolve */ }
                    }
                }
            } catch (_: Exception) {
                // Connection dropped — treat as cancel.
            } finally {
                if (!clientAck.isCompleted) clientAck.complete(false)
            }
        }

        // Pair succeeds if EITHER side approves. Fails only when both
        // explicitly decline or the 60s timeout elapses with no approval.
        val anyApproved = CompletableDeferred<Boolean>()
        session.launch {
            try {
                if (phoneAck.await()) anyApproved.complete(true)
            } catch (_: Exception) {}
        }
        session.launch {
            try {
                if (clientAck.await()) anyApproved.complete(true)
            } catch (_: Exception) {}
        }
        session.launch {
            // If both explicitly cancel, fail immediately without waiting
            // for the timeout.
            try {
                val phoneOk = phoneAck.await()
                val clientOk = clientAck.await()
                if (!phoneOk && !clientOk && !anyApproved.isCompleted) {
                    anyApproved.complete(false)
                }
            } catch (_: Exception) {}
        }
        val result = withTimeoutOrNull(60_000) { anyApproved.await() } ?: run {
            config.pairingManager.userCancel()
            false
        }
        phoneJob.cancel()
        readerJob.cancel()

        if (result) {
            val token = config.issuePairingToken(request.desktop_user, request.desktop_host)
            config.onEvent("pair OK — issued token for ${request.desktop_user}@${request.desktop_host}")
            config.onPairSuccess(config.deviceName())
            send(
                session,
                ServerMessage.PairOk(
                    token = token,
                    device_id = config.deviceId(),
                    device_name = config.deviceName(),
                    music_root = config.musicRootDisplay().trimEnd('/') + "/",
                ),
            )
        } else {
            config.onEvent("pair cancelled or timed out")
            send(session, ServerMessage.PairCancelled("user did not confirm in time"))
        }
        session.close(CloseReason(CloseReason.Codes.NORMAL, "pair done"))
    }

    private suspend fun handleFilePut(
        session: io.ktor.server.websocket.DefaultWebSocketServerSession,
        msg: ClientMessage.FilePut,
    ) {
        val safePath = validateRelativePath(msg.path)
        if (safePath == null) {
            send(session, ServerMessage.FileErr(msg.path, "invalid path"))
            return
        }
        val root = config.musicRoot()
        if (root == null) {
            send(session, ServerMessage.FileErr(msg.path, "phone has no music folder set"))
            return
        }
        try {
            val binary = nextBinary(session)
                ?: throw IllegalStateException("expected binary frame")
            if (binary.size.toLong() != msg.size) {
                throw IllegalStateException(
                    "size mismatch: announced=${msg.size}, received=${binary.size}",
                )
            }
            val data = binary
            AtomicFileWriter.writeStream(
                root = root,
                relPath = safePath,
                size = msg.size,
                reader = object : AtomicFileWriter.ByteReader {
                    var pos = 0
                    override fun read(buf: ByteArray, off: Int, len: Int): Int {
                        val avail = data.size - pos
                        if (avail <= 0) return -1
                        val n = minOf(len, avail)
                        System.arraycopy(data, pos, buf, off, n)
                        pos += n
                        return n
                    }
                },
                resolver = config.contentResolver,
            )
            config.onEvent("wrote ${safePath} (${msg.size} bytes)")
            send(session, ServerMessage.FileOk(msg.path))
        } catch (e: Exception) {
            send(session, ServerMessage.FileErr(msg.path, e.message ?: "write failed"))
        }
    }

    private suspend fun handlePlaylistPut(
        session: io.ktor.server.websocket.DefaultWebSocketServerSession,
        msg: ClientMessage.PlaylistPut,
    ) {
        val safeName = validateRelativePath(msg.name)
        if (safeName == null) {
            send(session, ServerMessage.PlaylistErr(msg.name, "invalid name"))
            return
        }
        val root = config.musicRoot()
        if (root == null) {
            send(session, ServerMessage.PlaylistErr(msg.name, "phone has no music folder set"))
            return
        }
        try {
            AtomicFileWriter.writeText(root, safeName, msg.content, config.contentResolver)
            config.onEvent("wrote playlist $safeName")
            send(session, ServerMessage.PlaylistOk(msg.name))
        } catch (e: Exception) {
            send(session, ServerMessage.PlaylistErr(msg.name, e.message ?: "write failed"))
        }
    }

    private suspend fun handleFileDelete(
        session: io.ktor.server.websocket.DefaultWebSocketServerSession,
        msg: ClientMessage.FileDelete,
    ) {
        val safePath = validateRelativePath(msg.path)
        if (safePath == null) {
            send(session, ServerMessage.FileDeleteErr(msg.path, "invalid path"))
            return
        }
        val root = config.musicRoot()
        if (root == null) {
            send(session, ServerMessage.FileDeleteErr(msg.path, "phone has no music folder set"))
            return
        }
        try {
            // Walk to the file via SAF and delete. Missing files are NOT an
            // error — match the previous semantics (Ruby/FTP code returned
            // OK for "already gone" because the desired end state is the
            // same).
            val parts = safePath.split('/').filter { it.isNotEmpty() }
            var node: DocumentFile? = root
            for (segment in parts) {
                node = node?.findFile(segment)
                if (node == null) {
                    // Already gone.
                    send(session, ServerMessage.FileDeleteOk(msg.path))
                    return
                }
            }
            if (node?.delete() == true) {
                send(session, ServerMessage.FileDeleteOk(msg.path))
            } else {
                send(session, ServerMessage.FileDeleteErr(msg.path, "delete failed"))
            }
        } catch (e: Exception) {
            send(session, ServerMessage.FileDeleteErr(msg.path, e.message ?: "delete failed"))
        }
    }

    private suspend fun nextText(
        session: io.ktor.server.websocket.DefaultWebSocketServerSession,
    ): String? {
        for (frame in session.incoming) {
            if (frame is Frame.Text) return frame.readText()
            if (frame is Frame.Close) return null
            // ping/pong/binary skipped here; binary handled by nextBinary
        }
        return null
    }

    private suspend fun nextBinary(
        session: io.ktor.server.websocket.DefaultWebSocketServerSession,
    ): ByteArray? {
        for (frame in session.incoming) {
            if (frame is Frame.Binary) return frame.readBytes()
            if (frame is Frame.Close) return null
        }
        return null
    }

    private fun parseClient(text: String): ClientMessage =
        json.decodeFromString(ClientMessage.serializer(), text)

    private suspend fun send(
        session: io.ktor.server.websocket.DefaultWebSocketServerSession,
        msg: ServerMessage,
    ) {
        val text = json.encodeToString(ServerMessage.serializer(), msg)
        session.outgoing.send(Frame.Text(text))
    }
}

// validateRelativePath lives in AtomicFileWriter.kt now.
