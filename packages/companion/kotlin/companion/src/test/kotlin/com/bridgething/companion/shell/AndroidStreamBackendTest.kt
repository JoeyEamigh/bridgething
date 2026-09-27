package com.bridgething.companion.shell

import androidx.media3.common.C
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import uniffi.bridgething_companion.StreamPresentation
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
    fun aPresentationBecomesTheSessionMetadataTheNotificationShows() {
        val metadata = presentedMetadataFor(
            StreamPresentation(
                title = "Blue in Green",
                artist = "Miles Davis",
                album = null,
                artwork = byteArrayOf(1, 2, 3),
            ),
        )
        assertEquals("Blue in Green", metadata.title.toString())
        assertEquals("Miles Davis", metadata.artist.toString())
        assertNull(metadata.albumTitle)
        assertArrayEquals(byteArrayOf(1, 2, 3), metadata.artworkData)
        assertEquals(MediaMetadata.PICTURE_TYPE_FRONT_COVER, metadata.artworkDataType)

        val bare = presentedMetadataFor(StreamPresentation(title = "Radio", artist = null, album = null, artwork = null))
        assertNull(bare.artworkData)
    }

    @Test
    fun thePlayerEchoingAPresentationIsNotReportedAsStreamMetadata() {
        val shown = presentedMetadataFor(StreamPresentation("Radio", "Host", null, byteArrayOf(9)))
        assertTrue(isEchoOf(presentedMetadataFor(StreamPresentation("Radio", "Host", null, byteArrayOf(9))), shown))
        assertFalse(isEchoOf(MediaMetadata.Builder().setTitle("Radio").setArtist("Host").build(), shown))
        assertFalse(isEchoOf(MediaMetadata.Builder().setTitle("Song").build(), shown))
        assertFalse(isEchoOf(shown, null))
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
        assertNull(mapped.artwork)
    }

    @Test
    fun embeddedArtworkBytesRideAlongWithTheTags() {
        val cover = byteArrayOf(0x89.toByte(), 0x50, 0x4E, 0x47)
        val metadata = MediaMetadata.Builder()
            .setTitle("Song")
            .setArtworkData(cover, MediaMetadata.PICTURE_TYPE_FRONT_COVER)
            .build()
        val mapped = streamMetadataFor(metadata)
        assertArrayEquals(cover, mapped.artwork)

        val bare = streamMetadataFor(MediaMetadata.Builder().setArtworkData(ByteArray(0), null).build())
        assertNull(bare.artwork)
    }

    @Test
    fun metadataFallsBackToTheStationName() {
        val metadata = MediaMetadata.Builder().setStation("Radio").build()
        val mapped = streamMetadataFor(metadata)
        assertEquals("Radio", mapped.title)
        assertNull(mapped.artist)
    }
}
