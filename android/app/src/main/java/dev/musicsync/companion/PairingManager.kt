package dev.musicsync.companion

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.security.SecureRandom

/**
 * Tracks an in-flight handshake awaiting phone-user resolution.
 *
 * Two flavours:
 *  - `requestPair`: bluetooth-style PAIR_REQUEST with a 6-digit code the
 *    user verifies matches the desktop. Confirm/Cancel UI.
 *  - `requestApproval`: HELLO arrived with an unrecognised token. The
 *    user just gets an Approve/Deny prompt — no code (the token itself
 *    is the secret).
 *
 * Only one prompt at a time; a second request supersedes the first.
 */
class PairingManager {

    data class Pending(
        /** Non-null = bluetooth-style with 6-digit code. Null = approve-only. */
        val code: String?,
        val deviceName: String,
        val desktopUser: String,
        val desktopHost: String,
        private val ack: CompletableDeferred<Boolean>,
    ) {
        internal fun resolveAndDetach(confirmed: Boolean) = ack.complete(confirmed)
        internal suspend fun awaitInternal(): Boolean = ack.await()
    }

    private val _pending = MutableStateFlow<Pending?>(null)
    val pending: StateFlow<Pending?> = _pending

    suspend fun requestPair(
        code: String,
        deviceName: String,
        desktopUser: String,
        desktopHost: String,
    ): Boolean = runRequest(
        Pending(code, deviceName, desktopUser, desktopHost, CompletableDeferred()),
    )

    suspend fun requestApproval(
        deviceName: String,
        desktopUser: String,
        desktopHost: String,
    ): Boolean = runRequest(
        Pending(null, deviceName, desktopUser, desktopHost, CompletableDeferred()),
    )

    private suspend fun runRequest(pend: Pending): Boolean {
        _pending.value?.resolveAndDetach(false)
        _pending.value = pend
        return try {
            pend.awaitInternal()
        } finally {
            _pending.compareAndSet(pend, null)
        }
    }

    fun userConfirm() { _pending.value?.resolveAndDetach(true) }
    fun userCancel() { _pending.value?.resolveAndDetach(false) }

    companion object {
        private val rng = SecureRandom()
        fun generateCode(): String {
            val n = rng.nextInt(1_000_000)
            return n.toString().padStart(6, '0')
        }
    }
}
