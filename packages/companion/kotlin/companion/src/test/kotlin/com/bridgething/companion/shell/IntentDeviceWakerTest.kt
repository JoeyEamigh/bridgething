package com.bridgething.companion.shell

import android.content.Context
import android.view.KeyEvent
import io.mockk.every
import io.mockk.mockk
import io.mockk.verify
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test
import uniffi.bridgething_companion.WakeReason

class IntentDeviceWakerTest {
    @Test
    fun everyReasonBroadcastsTheMediaKeyPair() {
        val context = mockk<Context>(relaxed = true)
        every { context.applicationContext } returns context

        val waker = IntentDeviceWaker(context)
        waker.wakeDevice(WakeReason.USER_PLAY, allowPlayTap = true)
        waker.wakeDevice(WakeReason.CONNECT_RESUME, allowPlayTap = true)

        verify(exactly = 4) { context.sendBroadcast(any()) }
    }

    @Test
    fun aTapPermittedWakeSendsPlayAndALaunchOnlyWakeSendsPause() {
        assertEquals(KeyEvent.KEYCODE_MEDIA_PLAY, IntentDeviceWaker.keyCodeFor(allowPlayTap = true))
        assertEquals(KeyEvent.KEYCODE_MEDIA_PAUSE, IntentDeviceWaker.keyCodeFor(allowPlayTap = false))
    }
}
