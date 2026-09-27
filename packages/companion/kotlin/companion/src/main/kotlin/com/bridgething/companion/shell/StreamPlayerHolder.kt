package com.bridgething.companion.shell

import android.content.Context
import android.os.Looper
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer

internal object StreamPlayerHolder {
    private var player: ExoPlayer? = null

    @androidx.annotation.OptIn(markerClass = [UnstableApi::class])
    fun obtain(context: Context): ExoPlayer {
        check(Looper.myLooper() == Looper.getMainLooper()) { "the stream player is main-looper affine" }
        player?.let { return it }
        val attributes = AudioAttributes.Builder()
            .setUsage(C.USAGE_MEDIA)
            .setContentType(C.AUDIO_CONTENT_TYPE_MUSIC)
            .build()
        val created = ExoPlayer.Builder(context.applicationContext)
            .setAudioAttributes(attributes, true)
            .setHandleAudioBecomingNoisy(true)
            .build()
        player = created
        return created
    }

    fun current(): ExoPlayer? = player
}
