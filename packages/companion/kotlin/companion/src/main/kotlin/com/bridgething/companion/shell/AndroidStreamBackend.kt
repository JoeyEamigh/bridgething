package com.bridgething.companion.shell

import android.content.Context
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import uniffi.bridgething_companion.StreamBackend
import uniffi.bridgething_companion.StreamSink

/**
 * Plays raw http(s) media URLs (internet radio, podcast episodes) with ExoPlayer.
 * The Car Thing has no speaker, so a webapp that wants audible audio for a stream
 * URL hands it here and the phone plays it.
 */
public class AndroidStreamBackend(context: Context) : StreamBackend {
    private val appContext = context.applicationContext
    private val lock = Any()
    private var player: ExoPlayer? = null
    private var sink: StreamSink? = null

    private val listener = object : Player.Listener {
        override fun onPlaybackStateChanged(playbackState: Int) {
            val sink = synchronized(lock) { sink } ?: return
            when (playbackState) {
                Player.STATE_READY -> runCatching { sink.onStarted() }
                Player.STATE_ENDED -> finish(error = null)
            }
        }

        override fun onPlayerError(error: PlaybackException) {
            finish(error = error.message ?: "playback failed")
        }
    }

    override fun play(url: String, sink: StreamSink) {
        stop()
        val player = ExoPlayer.Builder(appContext).build()
        synchronized(lock) {
            this.player = player
            this.sink = sink
        }
        player.addListener(listener)
        player.setMediaItem(MediaItem.fromUri(url))
        player.prepare()
        player.play()
    }

    override fun pause() {
        synchronized(lock) { player }?.pause()
    }

    override fun resume() {
        synchronized(lock) { player }?.play()
    }

    override fun stop() {
        finish(error = null)
    }

    private fun finish(error: String?) {
        val (player, sink) = synchronized(lock) {
            val player = this.player
            val sink = this.sink
            this.player = null
            this.sink = null
            player to sink
        }
        player?.removeListener(listener)
        runCatching { player?.stop() }
        runCatching { player?.release() }
        if (sink != null) runCatching { sink.onStopped(error) }
    }
}
