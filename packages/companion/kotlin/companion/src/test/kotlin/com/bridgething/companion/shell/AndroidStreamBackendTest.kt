package com.bridgething.companion.shell

import androidx.media3.common.C
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Test
import uniffi.bridgething_companion.StreamStatus

class AndroidStreamBackendTest {
    @Test
    fun playingWinsOverEveryOtherState() {
        assertEquals(StreamStatus.Playing, streamStatusFor(Player.STATE_READY, isPlaying = true))
        assertEquals(StreamStatus.Playing, streamStatusFor(Player.STATE_BUFFERING, isPlaying = true))
    }

    @Test
    fun bufferingAndEndedFollowThePlaybackState() {
        assertEquals(StreamStatus.Buffering, streamStatusFor(Player.STATE_BUFFERING, isPlaying = false))
        assertEquals(StreamStatus.Ended, streamStatusFor(Player.STATE_ENDED, isPlaying = false))
    }

    @Test
    fun readyButNotPlayingIsPaused() {
        assertEquals(StreamStatus.Paused, streamStatusFor(Player.STATE_READY, isPlaying = false))
    }

    @Test
    fun idleReportsNothing() {
        assertNull(streamStatusFor(Player.STATE_IDLE, isPlaying = false))
    }

    @Test
    fun anUnsetDurationMeansALiveStream() {
        val timing = streamTimingFor(positionMs = 4_200L, durationMs = C.TIME_UNSET, seekable = false, live = false)
        assertEquals(4_200u, timing.positionMs)
        assertNull(timing.durationMs)
        assertEquals(false, timing.seekable)
    }

    @Test
    fun aKnownDurationRidesAlongWithTheSeekableFlag() {
        val timing = streamTimingFor(positionMs = 1_000L, durationMs = 240_000L, seekable = true, live = false)
        assertEquals(1_000u, timing.positionMs)
        assertEquals(240_000u, timing.durationMs)
        assertEquals(true, timing.seekable)
    }

    @Test
    fun aNegativePositionClampsToZero() {
        val timing =
            streamTimingFor(positionMs = C.TIME_UNSET, durationMs = C.TIME_UNSET, seekable = false, live = false)
        assertEquals(0u, timing.positionMs)
    }

    @Test
    fun aLiveStreamHidesTheFakeContentLengthDuration() {
        val timing = streamTimingFor(positionMs = 9_000L, durationMs = 64_800_000L, seekable = true, live = true)
        assertEquals(9_000u, timing.positionMs)
        assertNull(timing.durationMs)
        assertEquals(false, timing.seekable)
    }

    @Test
    fun theStationNameSeedsTheTitleBeforeThePlayerHasAnyMetadata() {
        val seeded = initialMetadataFor("Groove Salad", metadataSeen = false)
        assertEquals("Groove Salad", seeded?.title)
        assertNull(seeded?.artist)
        assertNull(initialMetadataFor("Groove Salad", metadataSeen = true))
        assertNull(initialMetadataFor(null, metadataSeen = false))
        assertNull(initialMetadataFor("  ", metadataSeen = false))
    }

    @Test
    fun metadataPrefersTheIcyTitle() {
        val metadata = MediaMetadata.Builder()
            .setTitle("Song")
            .setStation("Radio")
            .setArtist("Band")
            .setAlbumTitle("Record")
            .build()
        val mapped = streamMetadataFor(metadata)
        assertEquals("Song", mapped.title)
        assertEquals("Band", mapped.artist)
        assertEquals("Record", mapped.album)
        assertNull(mapped.artworkUrl)
    }

    @Test
    fun metadataFallsBackToTheStationName() {
        val metadata = MediaMetadata.Builder().setStation("Radio").build()
        val mapped = streamMetadataFor(metadata)
        assertEquals("Radio", mapped.title)
        assertNull(mapped.artist)
    }
}
