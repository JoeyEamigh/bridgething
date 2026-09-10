package com.bridgething.companion.shell

import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper
import androidx.core.content.ContextCompat
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import com.bridgething.companion.BridgethingStreamService
import uniffi.bridgething_companion.StreamBackend
import uniffi.bridgething_companion.StreamMetadata
import uniffi.bridgething_companion.StreamSink
import uniffi.bridgething_companion.StreamSource
import uniffi.bridgething_companion.StreamStatus
import uniffi.bridgething_companion.StreamTiming

internal fun streamStatusFor(
    playbackState: Int,
    isPlaying: Boolean,
): StreamStatus? = when {
    isPlaying -> StreamStatus.Playing
    playbackState == Player.STATE_BUFFERING -> StreamStatus.Buffering
    playbackState == Player.STATE_ENDED -> StreamStatus.Ended
    playbackState == Player.STATE_READY -> StreamStatus.Paused
    else -> null
}

internal fun streamTimingFor(
    positionMs: Long,
    durationMs: Long,
    seekable: Boolean,
    live: Boolean,
): StreamTiming = StreamTiming(
    positionMs = clampToUInt(positionMs),
    durationMs = if (live || durationMs == C.TIME_UNSET) null else clampToUInt(durationMs),
    seekable = seekable && !live,
)

internal fun initialMetadataFor(
    station: String?,
    metadataSeen: Boolean,
): StreamMetadata? = if (metadataSeen || station.isNullOrBlank()) {
    null
} else {
    StreamMetadata(title = station, artist = null, album = null, artworkUrl = null)
}

internal fun streamMetadataFor(metadata: MediaMetadata): StreamMetadata = StreamMetadata(
    title = (metadata.title ?: metadata.station)?.toString(),
    artist = metadata.artist?.toString(),
    album = metadata.albumTitle?.toString(),
    artworkUrl = metadata.artworkUri?.toString(),
)

private fun clampToUInt(value: Long): UInt = value.coerceIn(0L, UInt.MAX_VALUE.toLong()).toUInt()

public class AndroidStreamBackend(
    context: Context,
) : StreamBackend {
    private val appContext = context.applicationContext
    private val main = Handler(Looper.getMainLooper())

    private var sink: StreamSink? = null
    private var listening = false
    private var ticking = false
    private var live = false
    private var metadataSeen = false
    private var suppressIdle = false
    private var lastStatus: StreamStatus? = null

    private val listener = object : Player.Listener {
        override fun onIsPlayingChanged(isPlaying: Boolean) = report()

        override fun onPlaybackStateChanged(playbackState: Int) {
            if (playbackState == Player.STATE_IDLE) {
                if (suppressIdle) {
                    suppressIdle = false
                    return
                }
                emit(StreamStatus.Ended)
                finishPlayback()
                return
            }
            report()
        }

        override fun onPlayerError(error: PlaybackException) {
            suppressIdle = true
            emit(StreamStatus.Failed(error.message ?: error.errorCodeName))
            finishPlayback()
        }

        override fun onMediaMetadataChanged(mediaMetadata: MediaMetadata) {
            metadataSeen = true
            val held = sink ?: return
            runCatching { held.onMetadata(streamMetadataFor(mediaMetadata)) }
        }
    }

    private val tick = object : Runnable {
        override fun run() {
            if (!ticking) return
            val player = StreamPlayerHolder.current()
            val held = sink
            if (player != null && held != null) {
                val timing = streamTimingFor(
                    positionMs = player.currentPosition,
                    durationMs = player.duration,
                    seekable = player.isCurrentMediaItemSeekable,
                    live = live,
                )
                runCatching { held.onTiming(timing) }
            }
            main.postDelayed(this, TICK_INTERVAL_MS)
        }
    }

    override fun appBundle(): String = appContext.packageName

    override fun play(
        source: StreamSource,
        sink: StreamSink,
    ) {
        main.post {
            swapSink(sink)
            lastStatus = null
            live = source.live
            metadataSeen = false
            suppressIdle = false
            val player = StreamPlayerHolder.obtain(appContext)
            if (!listening) {
                player.addListener(listener)
                listening = true
            }
            startService()
            player.setMediaItem(MediaItem.fromUri(source.url))
            player.prepare()
            player.play()
            startTicking()
            initialMetadataFor(source.station, metadataSeen)?.let { initial ->
                runCatching { sink.onMetadata(initial) }
            }
        }
    }

    override fun pause() {
        main.post { StreamPlayerHolder.current()?.pause() }
    }

    override fun resume() {
        main.post { StreamPlayerHolder.current()?.play() }
    }

    override fun seekTo(positionMs: UInt) {
        main.post {
            if (live) return@post
            StreamPlayerHolder.current()?.seekTo(positionMs.toLong())
        }
    }

    override fun stop() {
        main.post {
            suppressIdle = true
            stopTicking()
            StreamPlayerHolder.current()?.let {
                it.stop()
                it.clearMediaItems()
            }
            stopService()
            lastStatus = null
            swapSink(null)
        }
    }

    private fun swapSink(next: StreamSink?) {
        val previous = sink
        sink = next
        if (previous !== next) previous?.close()
    }

    private fun report() {
        val player = StreamPlayerHolder.current() ?: return
        val status = streamStatusFor(player.playbackState, player.isPlaying) ?: return
        emit(status)
        if (status is StreamStatus.Ended) finishPlayback()
    }

    private fun emit(status: StreamStatus) {
        if (status == lastStatus) return
        lastStatus = status
        val held = sink ?: return
        runCatching { held.onStatus(status) }
    }

    private fun finishPlayback() {
        stopTicking()
        stopService()
    }

    private fun startTicking() {
        if (ticking) return
        ticking = true
        main.post(tick)
    }

    private fun stopTicking() {
        ticking = false
        main.removeCallbacks(tick)
    }

    private fun startService() {
        runCatching { ContextCompat.startForegroundService(appContext, serviceIntent()) }
    }

    private fun stopService() {
        runCatching { appContext.stopService(serviceIntent()) }
    }

    private fun serviceIntent(): Intent = Intent(appContext, BridgethingStreamService::class.java)

    private companion object {
        const val TICK_INTERVAL_MS = 1_000L
    }
}
