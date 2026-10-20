package com.bridgething

import android.app.Activity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertSame
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class PendingRequestsTest {
    private data class Screen(val name: String)

    private val pick = ActivityRequest.MEDIA_PICK.code
    private val pair = ActivityRequest.COMPANION_DEVICE.code
    private val chosen = ActivityResult(Activity.RESULT_OK, null)

    private fun CoroutineScope.open(
        pending: PendingRequests<Screen>,
        code: Int,
        owner: Screen?,
        launched: MutableList<Screen> = mutableListOf(),
    ): Deferred<ActivityResult> = async(start = CoroutineStart.UNDISPATCHED) {
        pending.await(code, owner) { launched += it }
    }

    private suspend fun refusal(block: suspend () -> Unit): String? {
        val error = runCatching { block() }.exceptionOrNull()
        assertTrue(error is IllegalStateException, "expected a refusal, got $error")
        return error?.message?.substringBefore(':')
    }

    @Test
    fun theLaunchingScreenFinishingCancelsOnlyItsOwnRequests() = runBlocking {
        val pending = PendingRequests<Screen>()
        val main = Screen("MainActivity")
        val mine = open(pending, pick, main)
        val theirs = open(pending, pair, Screen("SettingsActivity"))

        pending.destroyed(main, finishing = true)

        assertEquals(Activity.RESULT_CANCELED, mine.await().resultCode)
        assertFalse(theirs.isCompleted)
        assertFalse(pending.deliver(pick, chosen))
        assertTrue(pending.deliver(pair, chosen))
        assertSame(chosen, theirs.await())
    }

    @Test
    fun anUnrelatedScreenFinishingLeavesTheRequestOpenForItsResult() = runBlocking {
        val pending = PendingRequests<Screen>()
        val result = open(pending, pick, Screen("MainActivity"))

        pending.destroyed(Screen("ChromeTabsManagerActivity"), finishing = true)

        assertFalse(result.isCompleted)
        assertTrue(pending.deliver(pick, chosen))
        assertSame(chosen, result.await())
    }

    @Test
    fun aLateDestroyOfAnEarlierInstanceOfTheSameScreenLeavesTheNewRequestOpen() = runBlocking {
        val pending = PendingRequests<Screen>()
        val earlier = Screen("MainActivity")
        val fresh = Screen("MainActivity")
        val result = open(pending, pick, fresh)

        pending.destroyed(earlier, finishing = true)

        assertFalse(result.isCompleted)
        assertTrue(pending.deliver(pick, chosen))
        assertSame(chosen, result.await())
    }

    @Test
    fun theLaunchingScreenDestroyedWithoutFinishingStillGetsTheResult() = runBlocking {
        val pending = PendingRequests<Screen>()
        val main = Screen("MainActivity")
        val result = open(pending, pick, main)

        pending.destroyed(main, finishing = false)

        assertFalse(result.isCompleted)
        assertTrue(pending.deliver(pick, chosen))
        assertSame(chosen, result.await())
    }

    @Test
    fun aSecondRequestOfTheSameKindIsBusyAndLeavesTheFirstIntact() = runBlocking {
        val pending = PendingRequests<Screen>()
        val main = Screen("MainActivity")
        val first = open(pending, pick, main)

        assertEquals("busy", refusal { pending.await(pick, main) {} })

        assertFalse(first.isCompleted)
        assertTrue(pending.deliver(pick, chosen))
        assertSame(chosen, first.await())
    }

    @Test
    fun aRequestWhileTheDialogHidesEveryScreenIsBusyRatherThanUnsupported() = runBlocking {
        val pending = PendingRequests<Screen>()
        val first = open(pending, pick, Screen("MainActivity"))

        assertEquals("busy", refusal { pending.await(pick, null) {} })

        assertFalse(first.isCompleted)
        assertTrue(pending.deliver(pick, chosen))
        assertSame(chosen, first.await())
    }

    @Test
    fun aRequestWithNoScreenIsUnsupportedAndClaimsNothing() = runBlocking {
        val pending = PendingRequests<Screen>()
        val launched = mutableListOf<Screen>()

        assertEquals("unsupported", refusal { pending.await(pick, null) { launched += it } })

        assertTrue(launched.isEmpty())
        val main = Screen("MainActivity")
        val next = open(pending, pick, main, launched)
        assertEquals(listOf(main), launched)
        assertTrue(pending.deliver(pick, chosen))
        assertSame(chosen, next.await())
    }

    @Test
    fun aLaunchThatThrowsFreesTheSlot() = runBlocking {
        val pending = PendingRequests<Screen>()
        val main = Screen("MainActivity")

        val error = runCatching { pending.await(pick, main) { throw IllegalArgumentException("no picker") } }
        assertTrue(error.exceptionOrNull() is IllegalArgumentException)

        val next = open(pending, pick, main)
        assertTrue(pending.deliver(pick, chosen))
        assertSame(chosen, next.await())
    }

    @Test
    fun aCancelledWaitFreesTheSlotAndItsLateResultIsNotDelivered() = runBlocking {
        val pending = PendingRequests<Screen>()
        val main = Screen("MainActivity")
        val cancelled = open(pending, pick, main)

        cancelled.cancel()
        cancelled.join()

        assertFalse(pending.deliver(pick, chosen))
        val next = open(pending, pick, main)
        assertTrue(pending.deliver(pick, chosen))
        assertSame(chosen, next.await())
    }
}
