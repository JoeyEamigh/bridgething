package com.bridgething

import android.os.Build
import java.util.concurrent.atomic.AtomicBoolean

internal fun interface Timer {
    fun after(delayMs: Long, action: () -> Unit): () -> Unit
}

internal class ScanOutcome(
    timer: Timer,
    deadlineMs: Long?,
    private val settle: (nothingFound: Boolean) -> Unit,
) {
    private val decided = AtomicBoolean(false)
    private val disarm: () -> Unit = deadlineMs
        ?.let { timer.after(it) { if (decided.compareAndSet(false, true)) settle(true) } }
        ?: {}

    fun found(present: () -> Unit) {
        if (!decided.compareAndSet(false, true)) return
        disarm()
        present()
    }

    fun failed(nothingFound: Boolean) {
        decided.set(true)
        disarm()
        settle(nothingFound)
    }
}

internal fun silentScanDeadlineMs(sdk: Int): Long? =
    if (sdk < Build.VERSION_CODES.TIRAMISU) SYSTEM_SCAN_MS + SCAN_SLACK_MS else null

internal const val SYSTEM_SCAN_MS = 20_000L

private const val SCAN_SLACK_MS = 5_000L
