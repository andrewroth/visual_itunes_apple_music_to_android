package dev.musicsync.companion

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress

/**
 * Proactive "I'm here" sender. Iterates the desktops we've ever talked
 * to and sends an unsolicited UDP packet with our identity to each, so
 * desktops can pick us up without needing their broadcast probes to
 * survive the local network.
 *
 * Wire payload mirrors [DiscoveryResponder]'s reply (same parser on the
 * desktop side handles both):
 *   MUSICSYNC_HERE {"name":"<device>","id":"<uuid>","port":7800}\n
 *
 * Sent to UDP 7798 (ANNOUNCE_PORT on the desktop), distinct from the
 * 7799 discovery port so we don't bounce off our own DiscoveryResponder.
 */
class DesktopAnnouncer(
    private val store: RememberedDesktopsStore,
    private val deviceName: () -> String,
    private val deviceId: () -> String,
    private val mainPort: Int = DEFAULT_PORT,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    /**
     * Fires one round of announcements. Cheap (one UDP packet per
     * remembered desktop, ~80 bytes each). Safe to call repeatedly —
     * e.g. on app start AND on every Wi-Fi reconnect.
     */
    fun announceOnce() {
        val ips = store.list()
        if (ips.isEmpty()) return
        scope.launch {
            val name = deviceName().replace("\\", "\\\\").replace("\"", "\\\"")
            val id = deviceId().replace("\\", "\\\\").replace("\"", "\\\"")
            val payload =
                "$REPLY_PREAMBLE {\"name\":\"$name\",\"id\":\"$id\",\"port\":$mainPort}\n"
            val bytes = payload.toByteArray(Charsets.UTF_8)
            val sock = try {
                DatagramSocket()
            } catch (_: Exception) {
                return@launch
            }
            try {
                for (ip in ips) {
                    val addr = try {
                        InetAddress.getByName(ip)
                    } catch (_: Exception) { continue }
                    try {
                        sock.send(DatagramPacket(bytes, bytes.size, addr, ANNOUNCE_PORT))
                    } catch (_: Exception) {
                        // Per-target failures are normal (host offline);
                        // keep going. Don't surface to the user.
                    }
                }
            } finally {
                try { sock.close() } catch (_: Exception) { }
            }
        }
    }

    companion object {
        const val ANNOUNCE_PORT = 7798
        const val REPLY_PREAMBLE = "MUSICSYNC_HERE"
    }
}
