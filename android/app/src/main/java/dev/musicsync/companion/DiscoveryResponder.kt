package dev.musicsync.companion

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress

/**
 * UDP-broadcast discovery responder. Listens on the well-known discovery
 * port; any incoming packet whose payload starts with [PROBE_PREAMBLE]
 * gets a JSON reply identifying this device. Much more reliable than
 * mDNS through misbehaving home APs, which often silently drop multicast.
 *
 * Wire format:
 *   request:  bytes "MUSICSYNC_DISCOVER\n"
 *   response: bytes "MUSICSYNC_HERE {\"name\":\"<device>\",\"port\":7800}\n"
 */
class DiscoveryResponder(
    private val deviceName: () -> String,
    private val mainPort: Int = DEFAULT_PORT,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var socket: DatagramSocket? = null
    private var job: Job? = null

    fun start() {
        if (job != null) return
        val sock = try {
            DatagramSocket(DISCOVERY_PORT).apply { broadcast = true }
        } catch (e: Exception) {
            // Port might be busy from another app; nothing we can do.
            return
        }
        socket = sock
        job = scope.launch {
            val buf = ByteArray(2048)
            while (true) {
                val pkt = DatagramPacket(buf, buf.size)
                try {
                    sock.receive(pkt) // blocks
                } catch (_: Exception) {
                    break // socket closed
                }
                val payload = String(pkt.data, 0, pkt.length, Charsets.UTF_8).trim()
                if (!payload.startsWith(PROBE_PREAMBLE)) continue
                val name = deviceName().replace("\\", "\\\\").replace("\"", "\\\"")
                val replyText =
                    "$REPLY_PREAMBLE {\"name\":\"$name\",\"port\":$mainPort}\n"
                val replyBytes = replyText.toByteArray(Charsets.UTF_8)
                try {
                    sock.send(DatagramPacket(replyBytes, replyBytes.size, pkt.address, pkt.port))
                } catch (_: Exception) {
                    // Ignore individual send failures; keep listening.
                }
            }
        }
    }

    fun stop() {
        try { socket?.close() } catch (_: Exception) { }
        scope.cancel()
        socket = null
        job = null
    }

    companion object {
        const val DISCOVERY_PORT = 7799
        const val PROBE_PREAMBLE = "MUSICSYNC_DISCOVER"
        const val REPLY_PREAMBLE = "MUSICSYNC_HERE"
    }
}
