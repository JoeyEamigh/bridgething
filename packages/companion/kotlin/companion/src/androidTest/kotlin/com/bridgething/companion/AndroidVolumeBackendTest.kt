package com.bridgething.companion

import android.content.Context
import android.media.AudioManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.bridgething.companion.shell.AndroidVolumeBackend
import kotlin.math.abs
import kotlin.math.roundToInt
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AndroidVolumeBackendTest {
    private val context: Context
        get() = InstrumentationRegistry.getInstrumentation().targetContext

    @Test
    fun realSetVolumeMovesStreamVolume() {
        val backend = AndroidVolumeBackend(context)
        val audio = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
        val max = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC)

        backend.setVolume(0.5f)
        val expected = (0.5f * max).roundToInt()
        val actual = audio.getStreamVolume(AudioManager.STREAM_MUSIC)
        assertTrue(
            "stream volume should land near the requested level (expected ~$expected, got $actual of $max)",
            abs(actual - expected) <= 1,
        )
    }

    @Test
    fun snapshotReportsTheStreamTheBackendJustMoved() {
        val backend = AndroidVolumeBackend(context)
        val audio = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
        val max = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC)

        backend.setVolume(0.5f)
        val snapshot = backend.snapshot()
        val expected = audio.getStreamVolume(AudioManager.STREAM_MUSIC).toFloat() / max.toFloat()
        assertEquals("snapshot should read back the stream it just set", expected, snapshot.level, 0.01f)
    }

    @Test
    fun steppingUpRaisesTheStream() {
        val backend = AndroidVolumeBackend(context)
        val audio = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager

        backend.setVolume(0.5f)
        val before = audio.getStreamVolume(AudioManager.STREAM_MUSIC)
        backend.volumeUp()
        assertTrue(
            "volumeUp should raise the music stream (was $before)",
            audio.getStreamVolume(AudioManager.STREAM_MUSIC) > before,
        )
    }
}
