package com.bridgething

import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class ScanOutcomeTest {
    private class ManualTimer : Timer {
        private var now = 0L
        private val due = mutableListOf<Pair<Long, () -> Unit>>()

        override fun after(delayMs: Long, action: () -> Unit): () -> Unit {
            val entry = now + delayMs to action
            due += entry
            return { due.remove(entry) }
        }

        fun advance(ms: Long) {
            now += ms
            due.filter { it.first <= now }.forEach { entry ->
                due.remove(entry)
                entry.second()
            }
        }
    }

    private val timer = ManualTimer()
    private val settled = mutableListOf<Boolean>()
    private val presented = mutableListOf<Unit>()

    private fun scan(sdk: Int) = ScanOutcome(timer, silentScanDeadlineMs(sdk), settled::add)

    @Test
    fun aScanThatFindsNothingBeforeAndroid13SettlesNothingFoundOnceAfterTheSystemScanEnds() {
        scan(sdk = 31)

        timer.advance(SYSTEM_SCAN_MS)
        assertEquals(emptyList<Boolean>(), settled)

        timer.advance(60_000)
        assertEquals(listOf(true), settled)
    }

    @Test
    fun aDeviceFoundInTimePresentsThePickerAndDisarmsTheDeadline() {
        val outcome = scan(sdk = 31)

        timer.advance(SYSTEM_SCAN_MS - 1)
        outcome.found { presented += Unit }
        timer.advance(60_000)

        assertEquals(1, presented.size)
        assertEquals(emptyList<Boolean>(), settled)
    }

    @Test
    fun aDeviceFoundAfterTheDeadlineSettledIsIgnored() {
        val outcome = scan(sdk = 26)

        timer.advance(60_000)
        outcome.found { presented += Unit }

        assertEquals(listOf(true), settled)
        assertTrue(presented.isEmpty())
    }

    @Test
    fun aFailureSettlesCancelledAndTheDeadlineNeverFollows() {
        val outcome = scan(sdk = 32)

        outcome.failed(nothingFound = false)
        timer.advance(60_000)
        outcome.found { presented += Unit }

        assertEquals(listOf(false), settled)
        assertTrue(presented.isEmpty())
    }

    @Test
    fun aPickerDismissedAfterItWasPresentedStillSettlesCancelled() {
        val outcome = scan(sdk = 31)

        outcome.found { presented += Unit }
        outcome.failed(nothingFound = false)
        timer.advance(60_000)

        assertEquals(1, presented.size)
        assertEquals(listOf(false), settled)
    }

    @Test
    fun android13AndLaterLeaveTheWaitToTheSystemPicker() {
        assertNull(silentScanDeadlineMs(33))

        val outcome = scan(sdk = 37)
        timer.advance(10 * 60_000)
        assertEquals(emptyList<Boolean>(), settled)

        outcome.found { presented += Unit }
        assertEquals(1, presented.size)
    }

    @Test
    fun theSystemPickersOwnTimeoutSettlesNothingFound() {
        val outcome = scan(sdk = 33)

        outcome.found { presented += Unit }
        outcome.failed(nothingFound = true)

        assertEquals(listOf(true), settled)
    }
}
