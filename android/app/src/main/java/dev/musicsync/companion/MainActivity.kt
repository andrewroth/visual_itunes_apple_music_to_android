package dev.musicsync.companion

import android.content.ComponentName
import android.content.Intent
import android.content.ServiceConnection
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.ui.res.painterResource
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.sp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import java.net.NetworkInterface
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * Companion UI. Server auto-starts on launch — no Start button. The page
 * shows, in order:
 *   - Server status chip (green when listening)
 *   - Pairing banner (green when paired, with device name + timestamp)
 *   - Scan banner (yellow while scanning, green when complete with counts)
 *   - LAN address (so a user without mDNS can still type it in)
 *   - Recent event log
 *
 * Pair-confirm dialog overlays when the desktop initiates pairing.
 */
class MainActivity : ComponentActivity() {

    private var service: SyncService? = null
    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName, binder: IBinder) {
            val svc = (binder as SyncService.Binder).service
            service = svc
            runningState.value = svc.isRunning()
            logState.value = svc.recentEvents()
            svc.addListener(listener)

            // Bridge service-level flows into Compose state.
            lifecycleScope.launch {
                svc.pairingManager.pending.collectLatest { pendingPair.value = it }
            }
            lifecycleScope.launch {
                svc.lastPaired.collectLatest { lastPaired.value = it }
            }
            lifecycleScope.launch {
                svc.scanState.collectLatest { scanState.value = it }
            }
            lifecycleScope.launch {
                svc.deviceName.collectLatest { deviceNameState.value = it }
            }
            lifecycleScope.launch {
                svc.musicRoot.collectLatest { musicRootState.value = it }
            }
            lifecycleScope.launch {
                svc.syncActive.collectLatest { syncActiveState.value = it }
            }
            lifecycleScope.launch {
                svc.syncProgress.collectLatest { syncProgressState.value = it }
            }
            lifecycleScope.launch {
                svc.connectedClients.collectLatest { connectedClientsState.value = it }
            }
            lifecycleScope.launch {
                svc.searchActive.collectLatest { searchActiveState.value = it }
            }
            lifecycleScope.launch {
                svc.hasPairing.collectLatest { hasPairingState.value = it }
            }
            lifecycleScope.launch {
                svc.pairedList.collectLatest { pairedListState.value = it }
            }
        }
        override fun onServiceDisconnected(name: ComponentName) { service = null }
    }
    private val listener: (String) -> Unit = { msg ->
        runOnUiThread {
            logState.value = (logState.value + msg).takeLast(200)
            runningState.value = service?.isRunning() == true
        }
    }

    private val runningState = mutableStateOf(false)
    private val logState = mutableStateOf<List<String>>(emptyList())
    private val pendingPair = mutableStateOf<PairingManager.Pending?>(null)
    private val lastPaired = mutableStateOf<SyncService.PairedInfo?>(null)
    private val scanState = mutableStateOf<SyncService.ScanState>(SyncService.ScanState.Idle)
    private val deviceNameState = mutableStateOf("")
    private val musicRootState = mutableStateOf("")
    private val syncActiveState = mutableStateOf(false)
    private val syncProgressState = mutableStateOf<SyncService.SyncProgress?>(null)
    private val connectedClientsState = mutableStateOf<List<String>>(emptyList())
    private val searchActiveState = mutableStateOf(true)
    private val hasPairingState = mutableStateOf(false)
    private val pairedListState = mutableStateOf<List<PairedDesktop>>(emptyList())

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Auto-start the service. It calls startServer() in onCreate so the
        // user sees a running server before any clicks. No Start button.
        val intent = Intent(this, SyncService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            startForegroundService(intent)
        } else {
            startService(intent)
        }
        bindService(intent, connection, BIND_AUTO_CREATE)

        setContent {
            MaterialTheme {
                @OptIn(ExperimentalMaterial3Api::class)
                Scaffold(
                    topBar = {
                        TopAppBar(
                            title = { BrandTitle() },
                            actions = {
                                Image(
                                    painter = painterResource(id = R.drawable.logo),
                                    contentDescription = null,
                                    modifier = Modifier
                                        .padding(end = 12.dp)
                                        .height(30.dp),
                                )
                            },
                        )
                    },
                ) { padding ->
                    // Two-tab layout: "Main" (status, pairing, address)
                    // "Connection" (status / device-name / pairings /
                    // LAN address), "Transfer" (music folder / inventory
                    // / sync progress), and "Log" (event stream). Header
                    // (TopAppBar above) and Quit (below) stay pinned in
                    // all tabs.
                    var selectedTab by rememberSaveable { mutableStateOf(0) }
                    val tabTitles = listOf("Connection", "Transfer", "Log")
                    Column(
                        modifier = Modifier.padding(padding).fillMaxSize(),
                    ) {
                        TabRow(selectedTabIndex = selectedTab) {
                            tabTitles.forEachIndexed { index, title ->
                                Tab(
                                    selected = selectedTab == index,
                                    onClick = { selectedTab = index },
                                    text = { Text(title) },
                                )
                            }
                        }
                        // Tab body fills the remaining space above the
                        // Quit button. Both tabs use the same horizontal
                        // padding as before.
                        Column(
                            modifier = Modifier
                                .weight(1f)
                                .padding(16.dp)
                                .fillMaxWidth(),
                            verticalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            when (selectedTab) {
                                0 -> ConnectionTabContent(
                                    runningState = runningState.value,
                                    connectedClients = connectedClientsState.value,
                                    searchActive = searchActiveState.value,
                                    onResumeSearch = { service?.resumeSearch() },
                                    deviceName = deviceNameState.value,
                                    onRename = { service?.renameDevice(it) },
                                    hasPairing = hasPairingState.value,
                                    paired = pairedListState.value,
                                    onForgetOne = { token ->
                                        service?.forgetPairing(token)
                                    },
                                    onForgetAll = { service?.forgetAllPairings() },
                                    lanIp = currentLanIp(),
                                )
                                1 -> TransferTabContent(
                                    musicRoot = musicRootState.value,
                                    onMusicRootChange = { uri, flags ->
                                        service?.setMusicRoot(uri, flags)
                                    },
                                    syncActive = syncActiveState.value,
                                    syncProgress = syncProgressState.value,
                                    onStopSync = {
                                        service?.stopSync("stopped by user on phone")
                                    },
                                    scan = scanState.value,
                                )
                                2 -> LogCard(events = logState.value)
                            }
                        }
                        QuitAppButton(
                            modifier = Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
                            transferActive = syncActiveState.value,
                            onQuit = {
                                service?.stopServer()
                                finishAffinity()
                            },
                        )
                    }

                    // Modal pair-confirm dialog. Appears whenever the
                    // PairingManager has a pending request; dismisses on
                    // user choice (which resolves the deferred inside the
                    // manager). Server-side timeout also clears it.
                    val pending = pendingPair.value
                    if (pending != null) {
                        AlertDialog(
                            onDismissRequest = { service?.pairingManager?.userCancel() },
                            title = {
                                Text(
                                    if (pending.code != null) "Pair new desktop?"
                                    else "Found a Viamta Music Sync Desktop. Approve?"
                                )
                            },
                            text = {
                                Column {
                                    if (pending.code != null) {
                                        Text("Verify this code matches the one shown on the desktop:")
                                        Spacer(Modifier.height(12.dp))
                                        Text(
                                            pending.code,
                                            fontFamily = FontFamily.Monospace,
                                            fontWeight = FontWeight.Bold,
                                            style = MaterialTheme.typography.displayMedium,
                                        )
                                    } else {
                                        // Approve-only prompt for HELLO with an
                                        // unrecognised token. Show whatever
                                        // identity we have.
                                        val label = if (pending.desktopUser.isNotBlank() ||
                                                        pending.desktopHost.isNotBlank())
                                            "${pending.desktopUser}@${pending.desktopHost}"
                                        else
                                            "(unknown desktop)"
                                        Text(
                                            label,
                                            fontFamily = FontFamily.Monospace,
                                            fontWeight = FontWeight.SemiBold,
                                        )
                                    }
                                }
                            },
                            confirmButton = {
                                TextButton(onClick = { service?.pairingManager?.userConfirm() }) {
                                    Text(if (pending.code != null) "Confirm" else "Yes")
                                }
                            },
                            dismissButton = {
                                TextButton(onClick = { service?.pairingManager?.userCancel() }) {
                                    Text(if (pending.code != null) "Cancel" else "No")
                                }
                            },
                        )
                    }
                }
            }
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        service?.removeListener(listener)
        try { unbindService(connection) } catch (_: Exception) { }
    }
}

// ----- Banner colors. Material3's color scheme doesn't have a built-in
// "success green", so we mix our own. Soft greens/yellows keep the UI
// from looking alarming when nothing's wrong.
private val GreenBg = Color(0xFFD7F3DB)
private val GreenFg = Color(0xFF15602B)
private val YellowBg = Color(0xFFFFF4CC)
private val YellowFg = Color(0xFF5A4500)
private val GreyBg = Color(0xFFEFEFEF)
private val GreyFg = Color(0xFF555555)

/**
 * Top-bar branding block: "Viamta Music Sync" on top with a small
 * subtitle "Visual iTunes/Apple Music to Android" underneath. Each
 * first-letter (V, i, A, M, t, A — spells *Viamta*) is colored, the
 * rest is muted grey so the highlight pops.
 */
@Composable
private fun BrandTitle() {
    // First letters of each word, in order: V i A M t A.
    val viamtaColors = listOf(
        Color(0xFFE53935), // V — red
        Color(0xFFFB8C00), // i — orange
        Color(0xFF7CB342), // A — green
        Color(0xFF1E88E5), // M — blue
        Color(0xFF8E24AA), // t — purple
        Color(0xFFD81B60), // A — pink
    )
    // Indices of the highlighted letters in the subtitle phrase.
    // "Visual iTunes/Apple Music to Android"
    //  0      7      14    20    26 29
    val subtitle = "Visual iTunes/Apple Music to Android"
    val highlightIndices = listOf(0, 7, 14, 20, 26, 29)
    val muted = Color(0xFF6B6B6B)
    val subtitleAnnotated: AnnotatedString = buildAnnotatedString {
        for ((i, ch) in subtitle.withIndex()) {
            val hi = highlightIndices.indexOf(i)
            if (hi >= 0) {
                withStyle(SpanStyle(color = viamtaColors[hi], fontWeight = FontWeight.Bold)) {
                    append(ch)
                }
            } else {
                withStyle(SpanStyle(color = muted)) {
                    append(ch)
                }
            }
        }
    }
    Column {
        Text(
            "Viamta Music Sync",
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            subtitleAnnotated,
            fontSize = 10.sp,
        )
    }
}

@Composable
private fun ServerStatusChip(
    running: Boolean,
    connectedClients: List<String>,
    searchActive: Boolean,
    onResumeSearch: () -> Unit,
) {
    when {
        !running -> ChipBox(GreyBg, GreyFg, "○ Server stopped")
        connectedClients.isNotEmpty() -> {
            // Use the first label (multi-desktop is rare for this app).
            val label = "● Desktop ${connectedClients.first()} connected"
            ChipBox(GreenBg, GreenFg, label)
        }
        searchActive -> ChipBox(YellowBg, YellowFg, "⟳ Searching for desktop app…")
        else -> {
            OutlinedButton(
                onClick = onResumeSearch,
                colors = ButtonDefaults.outlinedButtonColors(contentColor = GreyFg),
            ) {
                Text("Search for desktop app")
            }
        }
    }
}

@Composable
private fun ChipBox(bg: Color, fg: Color, label: String) {
    Box(
        Modifier
            .clip(RoundedCornerShape(8.dp))
            .background(bg)
            .padding(horizontal = 12.dp, vertical = 8.dp),
    ) {
        Text(label, color = fg, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
private fun PairedBanner(
    hasPairing: Boolean,
    count: Int,
    paired: List<PairedDesktop>,
    onForgetOne: (String) -> Unit,
    onForgetAll: () -> Unit,
) {
    val approvedCount = paired.count { it.approved }
    if (approvedCount == 0) {
        Box(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(8.dp))
                .background(GreyBg)
                .padding(12.dp),
        ) {
            Text("No desktops approved yet.", color = GreyFg)
        }
        return
    }
    var manageOpen by rememberSaveable { mutableStateOf(false) }
    val label = if (approvedCount == 1)
        "✓ 1 desktop approved"
    else
        "✓ $approvedCount desktops approved"
    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(GreenBg)
            .padding(horizontal = 12.dp, vertical = 8.dp),
    ) {
        Column(modifier = Modifier.weight(1f)) {
            Text(label, color = GreenFg, fontWeight = FontWeight.SemiBold)
        }
        TextButton(onClick = { manageOpen = true }) {
            Text("Approvals…")
        }
    }
    if (manageOpen) {
        ManageApprovalsDialog(
            entries = paired,
            onForgetOne = onForgetOne,
            onForgetAll = onForgetAll,
            onDismiss = { manageOpen = false },
        )
    }
}

@Composable
private fun ManageApprovalsDialog(
    entries: List<PairedDesktop>,
    onForgetOne: (String) -> Unit,
    onForgetAll: () -> Unit,
    onDismiss: () -> Unit,
) {
    var confirmAll by rememberSaveable { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Approvals") },
        text = {
            if (entries.isEmpty()) {
                Text("None.")
            } else {
                LazyColumn(modifier = Modifier.heightIn(min = 80.dp, max = 320.dp)) {
                    items(entries) { p ->
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(vertical = 6.dp),
                        ) {
                            // Status glyph: green ✓ for approved, red ✕ for denied.
                            val statusColor = if (p.approved) GreenFg else Color(0xFFB00020)
                            val statusGlyph = if (p.approved) "✓" else "✕"
                            Text(
                                statusGlyph,
                                color = statusColor,
                                fontWeight = FontWeight.Bold,
                                modifier = Modifier.padding(end = 8.dp),
                            )
                            Column(modifier = Modifier.weight(1f)) {
                                Text(
                                    "${p.user}@${p.host}",
                                    fontFamily = FontFamily.Monospace,
                                    fontWeight = FontWeight.SemiBold,
                                )
                                val statusLabel = if (p.approved)
                                    "approved ${formatTime(p.pairedAtMs)}"
                                else
                                    "denied ${formatTime(p.pairedAtMs)}"
                                Text(
                                    statusLabel,
                                    style = MaterialTheme.typography.bodySmall,
                                    color = GreyFg,
                                )
                            }
                            // Remove from the list. For approved entries
                            // this revokes access; for denied entries it
                            // means we'll prompt again on next HELLO.
                            TextButton(
                                onClick = { onForgetOne(p.token) },
                                colors = ButtonDefaults.textButtonColors(
                                    contentColor = Color(0xFFB00020),
                                ),
                            ) {
                                Text("Remove")
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("Close") }
        },
        dismissButton = {
            if (entries.isNotEmpty()) {
                TextButton(
                    onClick = { confirmAll = true },
                    colors = ButtonDefaults.textButtonColors(
                        contentColor = Color(0xFFB00020),
                    ),
                ) { Text("Clear all") }
            }
        },
    )
    if (confirmAll) {
        AlertDialog(
            onDismissRequest = { confirmAll = false },
            title = { Text("Clear all approvals + denials?") },
            text = {
                Text(
                    "Every desktop will need to pair again. Music files " +
                    "on this phone are not touched."
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    onForgetAll(); confirmAll = false; onDismiss()
                }) { Text("Clear all") }
            },
            dismissButton = {
                TextButton(onClick = { confirmAll = false }) { Text("Cancel") }
            },
        )
    }
}

@Composable
private fun ScanBanner(state: SyncService.ScanState) {
    when (state) {
        is SyncService.ScanState.Idle -> {
            // Don't show anything when idle — keeps the screen quieter.
        }
        is SyncService.ScanState.Scanning -> {
            Box(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(8.dp))
                    .background(YellowBg)
                    .padding(12.dp),
            ) {
                Column(modifier = Modifier.fillMaxWidth()) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        CircularProgressIndicator(
                            modifier = Modifier.size(18.dp),
                            strokeWidth = 2.dp,
                            color = YellowFg,
                        )
                        Spacer(Modifier.width(12.dp))
                        Text(
                            "Scanning music folder…",
                            color = YellowFg,
                            fontWeight = FontWeight.SemiBold,
                        )
                    }
                    Spacer(Modifier.height(8.dp))
                    // Determinate progress when we have a top-level count
                    // (the common case). Falls back to indeterminate
                    // while we're still listing the top level or when the
                    // user picked a flat folder with no subdirectories.
                    if (state.topLevelTotal > 0) {
                        val frac = state.topLevelDone.toFloat() /
                                   state.topLevelTotal.toFloat()
                        LinearProgressIndicator(
                            progress = { frac.coerceIn(0f, 1f) },
                            modifier = Modifier.fillMaxWidth().height(6.dp),
                            color = YellowFg,
                            trackColor = Color(0xFFFFE49C),
                        )
                    } else {
                        LinearProgressIndicator(
                            modifier = Modifier.fillMaxWidth().height(6.dp),
                            color = YellowFg,
                            trackColor = Color(0xFFFFE49C),
                        )
                    }
                    Spacer(Modifier.height(4.dp))
                    Text(
                        if (state.topLevelTotal > 0)
                            "${state.filesSoFar} files indexed — " +
                            "${state.topLevelDone} of ${state.topLevelTotal} folders done"
                        else
                            "${state.filesSoFar} files indexed so far",
                        color = YellowFg,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }
        is SyncService.ScanState.Complete -> {
            Box(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(8.dp))
                    .background(GreenBg)
                    .padding(12.dp),
            ) {
                Column {
                    Text(
                        "✓ Inventory complete: ${state.files} tracks, ${state.playlists} playlists",
                        color = GreenFg,
                        fontWeight = FontWeight.SemiBold,
                    )
                    Text(
                        "at ${formatTime(state.timestampMs)} — " +
                        "took ${formatDuration(state.durationMs)}",
                        color = GreenFg,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }
    }
}

@Composable
private fun DeviceNameRow(name: String, onRename: (String) -> Unit) {
    var dialogOpen by rememberSaveable { mutableStateOf(false) }
    var draft by rememberSaveable { mutableStateOf("") }

    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(GreyBg)
            .padding(horizontal = 12.dp, vertical = 8.dp),
    ) {
        Column(modifier = Modifier.weight(1f)) {
            Text(
                "Phone name",
                style = MaterialTheme.typography.bodySmall,
                color = GreyFg,
            )
            Text(
                name.ifBlank { "(loading)" },
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
                color = GreenFg,
            )
        }
        TextButton(onClick = {
            draft = name
            dialogOpen = true
        }) {
            Text("Rename")
        }
    }

    if (dialogOpen) {
        AlertDialog(
            onDismissRequest = { dialogOpen = false },
            title = { Text("Rename this phone") },
            text = {
                Column {
                    Text(
                        "This name appears on the desktop's discovery list " +
                        "and inside pairing screens. Make it something you'll " +
                        "recognise from a different machine."
                    )
                    Spacer(Modifier.height(12.dp))
                    OutlinedTextField(
                        value = draft,
                        onValueChange = { draft = it },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions.Default,
                        label = { Text("Name") },
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    onRename(draft)
                    dialogOpen = false
                }) { Text("Save") }
            },
            dismissButton = {
                TextButton(onClick = { dialogOpen = false }) { Text("Cancel") }
            },
        )
    }
}

@Composable
private fun SyncProgressBanner(progress: SyncService.SyncProgress?) {
    Box(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(YellowBg)
            .padding(12.dp),
    ) {
        Column(modifier = Modifier.fillMaxWidth()) {
            Text(
                "Transfer in progress",
                color = YellowFg,
                fontWeight = FontWeight.SemiBold,
            )
            Spacer(Modifier.height(8.dp))
            if (progress?.fraction != null) {
                LinearProgressIndicator(
                    progress = { progress.fraction.coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth().height(6.dp),
                    color = YellowFg,
                    trackColor = Color(0xFFFFE49C),
                )
            } else {
                LinearProgressIndicator(
                    modifier = Modifier.fillMaxWidth().height(6.dp),
                    color = YellowFg,
                    trackColor = Color(0xFFFFE49C),
                )
            }
            Spacer(Modifier.height(4.dp))
            Text(
                progress?.message ?: "Waiting for desktop progress…",
                color = YellowFg,
                style = MaterialTheme.typography.bodySmall,
            )
            if (progress?.fraction != null) {
                Text(
                    "${(progress.fraction * 100f).toInt()}%",
                    color = YellowFg,
                    style = MaterialTheme.typography.bodySmall,
                    fontWeight = FontWeight.SemiBold,
                )
            }
        }
    }
}

@Composable
private fun StopSyncButton(onStop: () -> Unit) {
    var confirm by rememberSaveable { mutableStateOf(false) }
    Button(
        onClick = { confirm = true },
        colors = ButtonDefaults.buttonColors(containerColor = Color(0xFFB00020)),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Text("Stop sync now")
    }
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Stop sync now?") },
            text = {
                Text(
                    "The desktop's transfer will abort. Already-copied files " +
                    "stay on the phone; the next sync will figure out what's " +
                    "still missing."
                )
            },
            confirmButton = {
                TextButton(onClick = { onStop(); confirm = false }) {
                    Text("Stop")
                }
            },
            dismissButton = {
                TextButton(onClick = { confirm = false }) { Text("Cancel") }
            },
        )
    }
}

@Composable
private fun QuitAppButton(
    transferActive: Boolean,
    onQuit: () -> Unit,
    modifier: Modifier = Modifier,
) {
    var confirm by rememberSaveable { mutableStateOf(false) }
    OutlinedButton(
        onClick = { confirm = true },
        colors = ButtonDefaults.outlinedButtonColors(contentColor = Color(0xFFB00020)),
        modifier = modifier.fillMaxWidth(),
    ) {
        Text("Quit app")
    }
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Quit Viamta Music Sync?") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text("This will stop the Viamta Music Sync companion app and close its background server.")
                    if (transferActive) {
                        Text(
                            "An active transfer is in progress. Quitting now will stop that transfer. " +
                            "Files already copied stay on the phone, and the next sync will resume from what's still missing.",
                        )
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { onQuit(); confirm = false }) {
                    Text("Quit")
                }
            },
            dismissButton = {
                TextButton(onClick = { confirm = false }) { Text("Cancel") }
            },
        )
    }
}

@Composable
private fun MusicRootRow(
    path: String,
    onChange: (Uri, Int) -> String?,
    disabled: Boolean = false,
) {
    var error by rememberSaveable { mutableStateOf<String?>(null) }

    // Storage Access Framework folder picker. The result is a tree URI
    // granting read+write to the chosen folder and all its subdirectories.
    // The grant flags from the launch intent are persisted by
    // SyncService → MusicRootStore via takePersistableUriPermission so
    // it survives reboots.
    val folderPicker = rememberLauncherForActivityResult(
        ActivityResultContracts.OpenDocumentTree(),
    ) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        val flags = Intent.FLAG_GRANT_READ_URI_PERMISSION or
                    Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        val err = onChange(uri, flags)
        error = err
    }

    Column(modifier = Modifier.fillMaxWidth()) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(8.dp))
                .background(GreyBg)
                .padding(horizontal = 12.dp, vertical = 8.dp),
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    "Music folder",
                    style = MaterialTheme.typography.bodySmall,
                    color = GreyFg,
                )
                Text(
                    path.ifBlank { "(not chosen)" },
                    style = MaterialTheme.typography.bodyMedium,
                    fontFamily = FontFamily.Monospace,
                )
            }
            TextButton(
                enabled = !disabled,
                onClick = {
                    error = null
                    folderPicker.launch(null)
                },
            ) {
                Text(
                    when {
                        disabled -> "Sync active"
                        path.isBlank() || path == "(not chosen)" -> "Pick folder…"
                        else -> "Change…"
                    }
                )
            }
        }
        error?.let {
            Spacer(Modifier.height(4.dp))
            Text(
                it,
                color = Color(0xFFB00020),
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(horizontal = 12.dp),
            )
        }
        if (path == "(not chosen)" || path.isBlank()) {
            Spacer(Modifier.height(4.dp))
            Text(
                "Pick the folder your music should live in. " +
                "Android blocks /sdcard root from picking, so pick a " +
                "subfolder like Music, Downloads, or anywhere else you " +
                "want.",
                style = MaterialTheme.typography.bodySmall,
                color = GreyFg,
                modifier = Modifier.padding(horizontal = 12.dp),
            )
        }
    }
}


@Composable
private fun AddressCard(ip: String?) {
    ElevatedCard(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            Text("LAN address", style = MaterialTheme.typography.titleSmall)
            Spacer(Modifier.height(4.dp))
            Text(
                if (ip != null) "ws://$ip:$DEFAULT_PORT" else "(no LAN address yet)",
                fontFamily = FontFamily.Monospace,
            )
            Text(
                "Desktop should auto-discover, but you can type this in if needed.",
                style = MaterialTheme.typography.bodySmall,
                color = GreyFg,
            )
        }
    }
}

/**
 * Body of the "Connection" tab: anything to do with who's talking to
 * whom — server status, the phone's display name, the list of paired
 * desktops, and the fallback LAN address card.
 */
@Composable
private fun ColumnScope.ConnectionTabContent(
    runningState: Boolean,
    connectedClients: List<String>,
    searchActive: Boolean,
    onResumeSearch: () -> Unit,
    deviceName: String,
    onRename: (String) -> Unit,
    hasPairing: Boolean,
    paired: List<PairedDesktop>,
    onForgetOne: (String) -> Unit,
    onForgetAll: () -> Unit,
    lanIp: String?,
) {
    ServerStatusChip(
        running = runningState,
        connectedClients = connectedClients,
        searchActive = searchActive,
        onResumeSearch = onResumeSearch,
    )
    DeviceNameRow(name = deviceName, onRename = onRename)
    PairedBanner(
        hasPairing = hasPairing,
        count = paired.size,
        paired = paired,
        onForgetOne = onForgetOne,
        onForgetAll = onForgetAll,
    )
    // While a desktop is connected, the LAN address card is just noise.
    if (connectedClients.isEmpty()) {
        AddressCard(ip = lanIp)
    }
}

/**
 * Body of the "Transfer" tab: the music-folder picker, the in-flight
 * sync banner + Stop button, and the scan/inventory summary.
 */
@Composable
private fun ColumnScope.TransferTabContent(
    musicRoot: String,
    onMusicRootChange: (android.net.Uri, Int) -> String?,
    syncActive: Boolean,
    syncProgress: SyncService.SyncProgress?,
    onStopSync: () -> Unit,
    scan: SyncService.ScanState,
) {
    MusicRootRow(
        path = musicRoot,
        onChange = onMusicRootChange,
        disabled = syncActive,
    )
    if (syncActive) {
        SyncProgressBanner(progress = syncProgress)
        StopSyncButton(onStop = onStopSync)
    }
    ScanBanner(state = scan)
}

@Composable
private fun ColumnScope.LogCard(events: List<String>) {
    val listState = rememberLazyListState()
    LaunchedEffect(events.size) {
        if (events.isNotEmpty()) {
            listState.animateScrollToItem(events.lastIndex)
        }
    }
    ElevatedCard(modifier = Modifier.fillMaxWidth().weight(1f)) {
        Column(Modifier.padding(16.dp).fillMaxSize()) {
            Text("Log", style = MaterialTheme.typography.titleSmall)
            Spacer(Modifier.height(8.dp))
            Box(modifier = Modifier.fillMaxSize()) {
                LazyColumn(
                    state = listState,
                    modifier = Modifier.fillMaxSize().padding(end = 8.dp),
                ) {
                    items(events) { e ->
                        Text(
                            e,
                            style = MaterialTheme.typography.bodySmall,
                            fontFamily = FontFamily.Monospace,
                        )
                    }
                }
                val total = listState.layoutInfo.totalItemsCount
                val visible = listState.layoutInfo.visibleItemsInfo.size
                if (total > visible && visible > 0) {
                    val thumbHeightFraction = (visible.toFloat() / total.toFloat())
                        .coerceIn(0.08f, 1f)
                    val maxOffsetFraction = (1f - thumbHeightFraction).coerceAtLeast(0f)
                    val startFraction = if (total <= visible) 0f else {
                        (listState.firstVisibleItemIndex.toFloat() /
                            (total - visible).toFloat()).coerceIn(0f, 1f) * maxOffsetFraction
                    }
                    Box(
                        modifier = Modifier
                            .align(Alignment.CenterEnd)
                            .fillMaxHeight()
                            .width(4.dp)
                            .clip(RoundedCornerShape(999.dp))
                            .background(Color(0x22000000)),
                    ) {
                        val endFraction = (1f - thumbHeightFraction - startFraction)
                            .coerceAtLeast(0f)
                        Column(modifier = Modifier.fillMaxSize()) {
                            if (startFraction > 0f) {
                                Spacer(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .weight(startFraction)
                                )
                            }
                            Box(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .weight(thumbHeightFraction)
                                    .background(Color(0x66000000), RoundedCornerShape(999.dp))
                            )
                            if (endFraction > 0f) {
                                Spacer(
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .weight(endFraction)
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

private fun currentLanIp(): String? {
    return try {
        NetworkInterface.getNetworkInterfaces().asSequence()
            .flatMap { ni -> ni.inetAddresses.asSequence() }
            .filter { !it.isLoopbackAddress && it.hostAddress?.contains('.') == true }
            .map { it.hostAddress }
            .firstOrNull()
    } catch (_: Exception) { null }
}

private val TIME_FMT = SimpleDateFormat("HH:mm:ss", Locale.getDefault())
private fun formatTime(ms: Long): String = TIME_FMT.format(Date(ms))

/** Compact elapsed-time string: "350ms", "12s", "1:30", "1:23:45". */
private fun formatDuration(ms: Long): String {
    if (ms < 1000) return "${ms}ms"
    val totalSec = ms / 1000
    if (totalSec < 60) return "${totalSec}s"
    val h = totalSec / 3600
    val m = (totalSec % 3600) / 60
    val s = totalSec % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, s)
    else "%d:%02d".format(m, s)
}
