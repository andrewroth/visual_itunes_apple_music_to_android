package dev.musicsync.companion

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList

/**
 * Foreground service hosting the WebSocket server. Keeps the server alive
 * while the user is away from the app, with a persistent notification so
 * Android doesn't kill the process.
 *
 * State that the UI observes (via [StateFlow]):
 *  - [lastPaired]: which desktop last completed pairing, plus when.
 *  - [scanState]: whether we are currently building a manifest, and the
 *    result of the most recent scan.
 *
 * The server reports these via callbacks plumbed through [SyncServer.Config].
 */
class SyncService : Service() {

    private var server: SyncServer? = null
    val pairingManager = PairingManager()
    private val advertiser by lazy { MdnsAdvertiser(applicationContext) }
    private val deviceNameStore by lazy { DeviceNameStore(applicationContext) }
    private val deviceIdStore by lazy { DeviceIdStore(applicationContext) }
    private val musicRootStore by lazy { MusicRootStore(applicationContext) }
    private val discoveryResponder by lazy {
        DiscoveryResponder(
            deviceName = { deviceNameStore.get() },
            deviceId = { deviceIdStore.get() },
        )
    }
    private val rememberedDesktopsStore by lazy {
        RememberedDesktopsStore(applicationContext)
    }
    private val desktopAnnouncer by lazy {
        DesktopAnnouncer(
            store = rememberedDesktopsStore,
            deviceName = { deviceNameStore.get() },
            deviceId = { deviceIdStore.get() },
            onEvent = { msg -> pushEvent(msg) },
        )
    }

    private val _deviceName = MutableStateFlow<String>("")
    val deviceName: StateFlow<String> = _deviceName

    private val _musicRoot = MutableStateFlow<String>("")
    val musicRoot: StateFlow<String> = _musicRoot

    /** Number of currently-active transfer sessions. Exposed as a boolean
     *  StateFlow for the UI to disable destructive controls. */
    private val activeSyncCount = java.util.concurrent.atomic.AtomicInteger(0)
    private val _syncActive = MutableStateFlow(false)
    val syncActive: StateFlow<Boolean> = _syncActive

    data class SyncProgress(
        val message: String,
        val fraction: Float? = null,
    )

    private val _syncProgress = MutableStateFlow<SyncProgress?>(null)
    val syncProgress: StateFlow<SyncProgress?> = _syncProgress

    /** Labels of currently-connected desktops (user@host). Empty list
     *  means nobody is connected. Drives the chip text + the address-
     *  card visibility. */
    private val _connectedClients = MutableStateFlow<List<String>>(emptyList())
    val connectedClients: StateFlow<List<String>> = _connectedClients

    /** True while we're actively expecting a desktop to find us. When
     *  [searchTimeoutMs] is negative, search stays active indefinitely. */
    private val _searchActive = MutableStateFlow(true)
    val searchActive: StateFlow<Boolean> = _searchActive

    private val searchTimeoutMs = -1L
    private var searchTimeoutJob: Job? = null
    private val serviceScope = CoroutineScope(Dispatchers.Default)

    private fun armSearchTimeout() {
        searchTimeoutJob?.cancel()
        _searchActive.value = true
        if (searchTimeoutMs < 0L) {
            searchTimeoutJob = null
            return
        }
        searchTimeoutJob = serviceScope.launch {
            delay(searchTimeoutMs)
            _searchActive.value = false
            pushEvent("No desktop connected after ${searchTimeoutMs / 1000L}s — search paused")
        }
    }

    private fun cancelSearchTimeout() {
        searchTimeoutJob?.cancel()
        searchTimeoutJob = null
    }

    /** Called from MainActivity when the user taps "Search for desktop
     *  app." Re-enables the searching state (and timer, if enabled). */
    fun resumeSearch() {
        armSearchTimeout()
        pushEvent("Search resumed")
    }

    data class PairedInfo(val deviceName: String, val timestampMs: Long)

    sealed class ScanState {
        data object Idle : ScanState()
        data class Scanning(
            val filesSoFar: Int = 0,
            val topLevelDone: Int = 0,
            val topLevelTotal: Int = 0,
        ) : ScanState()
        data class Complete(
            val files: Int,
            val playlists: Int,
            val timestampMs: Long,
            val durationMs: Long,
        ) : ScanState()
    }

    /** Wall-clock time the most recent scan started, for elapsed-time
     *  reporting. Set in onScanStarted, consumed in onScanComplete. */
    private var scanStartMs: Long = 0L

    /** Acquired during active transfers to keep the CPU + Wi-Fi alive
     *  even when the user has navigated away or the screen is off. Held
     *  by the foreground service so Android's Doze mode + power-save
     *  Wi-Fi don't throttle the in-flight upload. */
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null

    private fun acquireTransferLocks() {
        if (wakeLock?.isHeld == true) return // already holding
        val pm = getSystemService(POWER_SERVICE) as PowerManager
        wakeLock = pm.newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK,
            "musicsync:transfer",
        ).apply {
            setReferenceCounted(false)
            // 1-hour ceiling so a runaway lock never persists forever.
            acquire(60 * 60 * 1000L)
        }
        val wm = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        wifiLock = wm.createWifiLock(
            WifiManager.WIFI_MODE_FULL_HIGH_PERF,
            "musicsync:transfer",
        ).apply {
            setReferenceCounted(false)
            acquire()
        }
        pushEvent("acquired wake + wifi locks for transfer")
    }

    private fun releaseTransferLocks() {
        try { wakeLock?.takeIf { it.isHeld }?.release() } catch (_: Exception) {}
        try { wifiLock?.takeIf { it.isHeld }?.release() } catch (_: Exception) {}
        wakeLock = null
        wifiLock = null
    }

    private val _lastPaired = MutableStateFlow<PairedInfo?>(null)
    val lastPaired: StateFlow<PairedInfo?> = _lastPaired

    /** True whenever any token is currently stored — survives app restart,
     *  unlike [lastPaired] which is session-only. The UI uses this to
     *  show "Paired" vs "Not paired yet" without needing a fresh
     *  handshake to populate state. */
    private val _hasPairing = MutableStateFlow(false)
    val hasPairing: StateFlow<Boolean> = _hasPairing

    /** Full list of currently-paired desktops with their labels, for the
     *  Manage Pairings dialog. */
    private val _pairedList = MutableStateFlow<List<PairedDesktop>>(emptyList())
    val pairedList: StateFlow<List<PairedDesktop>> = _pairedList

    private val _scanState = MutableStateFlow<ScanState>(ScanState.Idle)
    val scanState: StateFlow<ScanState> = _scanState

    private val eventListeners = CopyOnWriteArrayList<(String) -> Unit>()
    private val eventLog = mutableListOf<String>()

    inner class Binder : android.os.Binder() {
        val service: SyncService get() = this@SyncService
    }
    private val binder = Binder()

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onCreate() {
        super.onCreate()
        ensureNotificationChannel()
        // Auto-start the server when the service is created. The activity
        // calls startForegroundService() in onCreate, so this fires before
        // the user sees any UI — there is no "Start" button to click.
        startServer()
    }

    fun startServer() {
        if (server != null) return
        val tokens = TokenStore(applicationContext)
        _deviceName.value = deviceNameStore.get()
        _musicRoot.value = musicRootStore.getDisplayPath()
        _hasPairing.value = tokens.hasAny()
        _pairedList.value = tokens.list()
        server = SyncServer(
            SyncServer.Config(
                musicRoot = { musicRootStore.getRoot() },
                musicRootDisplay = { musicRootStore.getDisplayPath() },
                contentResolver = applicationContext.contentResolver,
                deviceName = { deviceNameStore.get() },
                deviceId = { deviceIdStore.get() },
                verifyToken = { tokens.verify(it) },
                // Multi-pair: every successful pair appends a new entry
                // with the desktop's identifier. No rejection gate.
                issuePairingToken = { user, host -> tokens.addPairing(user, host) },
                requestApprovalForHello = { user, host ->
                    pairingManager.requestApproval(
                        deviceName = deviceNameStore.get(),
                        desktopUser = user.ifBlank { "unknown" },
                        desktopHost = host.ifBlank { "unknown" },
                    )
                },
                acceptExistingToken = { tok, user, host ->
                    tokens.acceptExistingToken(
                        tok,
                        user = user.ifBlank { "unknown" },
                        host = host.ifBlank { "unknown" },
                    )
                    _pairedList.value = tokens.list()
                    _hasPairing.value = true
                },
                pairingManager = pairingManager,
                onPairSuccess = { deviceName ->
                    _lastPaired.value = PairedInfo(deviceName, System.currentTimeMillis())
                    _hasPairing.value = true
                    _pairedList.value = tokens.list()
                },
                onScanStarted = {
                    scanStartMs = System.currentTimeMillis()
                    _scanState.value = ScanState.Scanning()
                },
                onScanProgress = { n, done, total ->
                    _scanState.value = ScanState.Scanning(n, done, total)
                },
                onScanComplete = { files, playlists ->
                    val now = System.currentTimeMillis()
                    _scanState.value = ScanState.Complete(
                        files = files,
                        playlists = playlists,
                        timestampMs = now,
                        durationMs = (now - scanStartMs).coerceAtLeast(0),
                    )
                },
                onSyncStarted = {
                    val n = activeSyncCount.incrementAndGet()
                    _syncActive.value = n > 0
                    if (n == 1) _syncProgress.value = SyncProgress("Preparing transfer…", null)
                    if (n == 1) acquireTransferLocks()
                },
                onSyncProgress = { message, fraction ->
                    _syncProgress.value = SyncProgress(message, fraction)
                },
                onSyncEnded = {
                    val n = activeSyncCount.decrementAndGet().coerceAtLeast(0)
                    _syncActive.value = n > 0
                    if (n == 0) _syncProgress.value = null
                    if (n == 0) releaseTransferLocks()
                },
                onClientsChanged = { labels ->
                    _connectedClients.value = labels
                    if (labels.isNotEmpty()) {
                        cancelSearchTimeout()
                        _searchActive.value = false
                    } else {
                        armSearchTimeout()
                    }
                },
                onClientAuthed = { ip ->
                    val before = rememberedDesktopsStore.list()
                    rememberedDesktopsStore.remember(ip)
                    // Only log when this is a NEW IP (or it moved to
                    // the head) — quiet on routine reconnects from the
                    // same desktop we've already remembered first.
                    if (before.firstOrNull() != ip) {
                        pushEvent("Remembered desktop IP $ip for proactive announce")
                    }
                },
                onEvent = { msg -> pushEvent(msg) },
            ),
        )
        startForeground(NOTIF_ID, buildNotification("listening"))
        server?.start()
        advertiser.register(deviceNameStore.get(), deviceIdStore.get(), DEFAULT_PORT)
        discoveryResponder.start()
        // Proactively tell desktops we've ever served that we're back
        // online. Unicast UDP — works on networks that filter broadcast
        // / multicast. Also re-fires on every reconnection (below).
        desktopAnnouncer.announceOnce("server start")
        registerNetworkCallback()
        armSearchTimeout()
    }

    private var networkCallback: android.net.ConnectivityManager.NetworkCallback? = null

    /**
     * Re-announce ourselves whenever the OS reports a new usable
     * network — Wi-Fi reconnect after the phone moved between APs,
     * VPN coming up, etc. Cheap enough to fire on every onAvailable
     * without rate-limiting; the announcer is ~80 bytes × N desktops.
     */
    private fun registerNetworkCallback() {
        if (networkCallback != null) return
        val cm = getSystemService(android.content.Context.CONNECTIVITY_SERVICE)
            as? android.net.ConnectivityManager ?: return
        val cb = object : android.net.ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: android.net.Network) {
                desktopAnnouncer.announceOnce("network up")
            }
        }
        try {
            cm.registerDefaultNetworkCallback(cb)
            networkCallback = cb
        } catch (_: Exception) { /* ignore — best effort */ }
    }

    private fun unregisterNetworkCallback() {
        val cb = networkCallback ?: return
        val cm = getSystemService(android.content.Context.CONNECTIVITY_SERVICE)
            as? android.net.ConnectivityManager ?: return
        try { cm.unregisterNetworkCallback(cb) } catch (_: Exception) { }
        networkCallback = null
    }

    /**
     * Change the music root directory. Validates + persists, updates the
     * UI state flow, and force-closes every active websocket so connected
     * desktops can't keep using stale manifest data. Returns null on
     * success or an error string for the UI.
     */
    /**
     * Save a freshly-picked SAF tree URI as the music root. [grantFlags]
     * are the Intent flags from the picker result. Returns null on
     * success or a user-facing error string.
     */
    fun setMusicRoot(uri: android.net.Uri, grantFlags: Int): String? {
        val err = musicRootStore.trySet(uri, grantFlags)
        if (err != null) return err
        _musicRoot.value = musicRootStore.getDisplayPath()
        pushEvent("Music folder set to ${musicRootStore.getDisplayPath()}")
        // Soft signal: don't kick existing sessions, the server reads the
        // new root via the closure on its next FILE_PUT / MANIFEST_REQUEST.
        return null
    }

    fun clearMusicRoot() {
        musicRootStore.clear()
        _musicRoot.value = musicRootStore.getDisplayPath()
        pushEvent("Music folder cleared")
    }

    /**
     * Abort all in-flight transfers by closing every active session.
     * Desktop receives a normal close → its current run_sync errors out →
     * UI flips back to idle. Called from the phone's "Stop sync" button.
     */
    fun stopSync(reason: String = "stopped by user on phone") {
        server?.closeAllSessions(reason)
        pushEvent("Sync stopped: $reason")
    }

    /**
     * Rename the phone: persists the new name, pushes DEVICE_RENAMED to
     * every live desktop session so their UI updates in place, and
     * re-advertises mDNS (the WebSocket server itself is NOT touched —
     * heartbeats keep flowing and there is no rescan on the desktop).
     *
     * Desktops match the phone by `device_id`, which is immutable, so a
     * rename never looks like a new device.
     */
    fun renameDevice(newName: String) {
        deviceNameStore.set(newName)
        val effective = deviceNameStore.get()
        _deviceName.value = effective
        // Notify any currently-connected desktops over the live socket
        // so they update their "paired with X" banner instantly. Done
        // BEFORE the mDNS bounce so the message goes out before the
        // browse-side dedup window blinks.
        server?.broadcastRename(deviceIdStore.get(), effective)
        // Re-advertise so a fresh desktop launch sees the new name in
        // discovery. mDNS instance name is device_id-derived, so this
        // doesn't look like a different service to the desktop.
        if (server != null) {
            advertiser.updateName(effective, deviceIdStore.get(), DEFAULT_PORT)
        }
        pushEvent("Renamed to \"$effective\"")
    }

    fun stopServer() {
        advertiser.unregister()
        discoveryResponder.stop()
        unregisterNetworkCallback()
        server?.stop()
        server = null
        cancelSearchTimeout()
        releaseTransferLocks() // defence in depth — should already be released
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    fun isRunning(): Boolean = server != null

    /**
     * Rotate the device's pairing token. Every desktop that was previously
     * paired with this phone will now fail HELLO and have to re-pair. Also
     * clears the in-process "last paired" banner.
     */
    fun forgetAllPairings() {
        TokenStore(applicationContext).clearAll()
        _lastPaired.value = null
        _hasPairing.value = false
        _pairedList.value = emptyList()
        pushEvent("All pairings forgotten — phone is open for a new pair")
    }

    /** Remove one specific paired desktop (from Manage Pairings X button). */
    fun forgetPairing(token: String) {
        val store = TokenStore(applicationContext)
        store.removeByToken(token)
        val remaining = store.list()
        _pairedList.value = remaining
        _hasPairing.value = remaining.isNotEmpty()
        if (remaining.isEmpty()) _lastPaired.value = null
        pushEvent("Removed one paired desktop; ${remaining.size} remain")
    }

    fun recentEvents(): List<String> = synchronized(eventLog) { eventLog.toList() }

    fun addListener(l: (String) -> Unit) {
        eventListeners.add(l)
    }
    fun removeListener(l: (String) -> Unit) {
        eventListeners.remove(l)
    }

    private fun pushEvent(msg: String) {
        val stamped = "${logTimeFmt.format(java.util.Date())}  $msg"
        synchronized(eventLog) {
            eventLog.add(stamped)
            if (eventLog.size > MAX_LOG) eventLog.removeAt(0)
        }
        eventListeners.forEach { it(stamped) }
    }

    /** "H:mm" timestamps prefix every log entry so the user can correlate
     *  events on the phone with what the desktop reports. */
    private val logTimeFmt = java.text.SimpleDateFormat("H:mm", java.util.Locale.getDefault())


    private fun ensureNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val mgr = getSystemService(NOTIFICATION_SERVICE) as NotificationManager
            val ch = NotificationChannel(
                CHANNEL_ID,
                "Viamta Music Sync server",
                NotificationManager.IMPORTANCE_LOW,
            )
            ch.description = "Background sync server"
            mgr.createNotificationChannel(ch)
        }
    }

    private fun buildNotification(text: String): Notification {
        val openApp = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("Viamta Music Sync")
            .setContentText(text)
            .setOngoing(true)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentIntent(openApp)
            .build()
    }

    companion object {
        private const val CHANNEL_ID = "musicsync"
        private const val NOTIF_ID = 1
        private const val MAX_LOG = 500
    }
}
