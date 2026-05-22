package dev.musicsync.companion

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo

/**
 * Registers the WebSocket server with the LAN via mDNS so the desktop can
 * discover us without typing an IP. Service type is "_musicsync._tcp.";
 * NsdManager appends ".local." for us.
 */
class MdnsAdvertiser(private val context: Context) {

    private var manager: NsdManager? = null
    private var listener: NsdManager.RegistrationListener? = null

    fun register(deviceName: String, deviceId: String, port: Int) {
        if (listener != null) return
        val mgr = context.getSystemService(Context.NSD_SERVICE) as NsdManager
        manager = mgr
        val info = NsdServiceInfo().apply {
            // Service instance name uses the device_id so renames don't
            // change the mDNS identity the desktop dedupes on. (Display
            // name still ships via the `name` TXT entry.)
            serviceName = "MusicSync ${deviceId.take(8)}"
            serviceType = "_musicsync._tcp."
            setPort(port)
            // Advertise the friendly device name via a TXT record so the
            // desktop can show it without resolving the hostname.
            setAttribute("name", deviceName)
            // Stable per-install id — the desktop matches on this, so
            // renames look like the same device.
            setAttribute("id", deviceId)
        }
        val l = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(s: NsdServiceInfo) {}
            override fun onRegistrationFailed(s: NsdServiceInfo, code: Int) {}
            override fun onServiceUnregistered(s: NsdServiceInfo) {}
            override fun onUnregistrationFailed(s: NsdServiceInfo, code: Int) {}
        }
        listener = l
        mgr.registerService(info, NsdManager.PROTOCOL_DNS_SD, l)
    }

    /**
     * Re-advertise with a new display name. Implemented as unregister +
     * register because NsdManager does not support TXT-only updates on
     * pre-Tiramisu devices; the service instance name is stable
     * (device_id-derived), so a fresh resolve still maps to the same
     * peer on the desktop. The WebSocket server is unaffected by this
     * — open sessions keep running.
     */
    fun updateName(newName: String, deviceId: String, port: Int) {
        unregister()
        register(newName, deviceId, port)
    }

    fun unregister() {
        val mgr = manager
        val l = listener
        if (mgr != null && l != null) {
            try { mgr.unregisterService(l) } catch (_: Exception) { /* idempotent */ }
        }
        manager = null
        listener = null
    }
}
