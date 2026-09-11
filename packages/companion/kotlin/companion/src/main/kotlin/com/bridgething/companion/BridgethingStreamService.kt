package com.bridgething.companion

import android.content.Intent
import android.os.Bundle
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.CommandButton
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionResult
import com.bridgething.companion.shell.StreamPlayerHolder
import com.google.common.collect.ImmutableList
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture

public class BridgethingStreamService : MediaSessionService() {
    private var session: MediaSession? = null

    private val stopCommand = SessionCommand(ACTION_STOP, Bundle.EMPTY)

    private val callback = object : MediaSession.Callback {
        override fun onConnect(
            session: MediaSession,
            controller: MediaSession.ControllerInfo,
        ): MediaSession.ConnectionResult =
            MediaSession.ConnectionResult.accept(
                MediaSession.ConnectionResult.DEFAULT_SESSION_COMMANDS.buildUpon().add(stopCommand).build(),
                MediaSession.ConnectionResult.DEFAULT_PLAYER_COMMANDS,
            )

        override fun onCustomCommand(
            session: MediaSession,
            controller: MediaSession.ControllerInfo,
            customCommand: SessionCommand,
            args: Bundle,
        ): ListenableFuture<SessionResult> {
            if (customCommand.customAction != ACTION_STOP) {
                return Futures.immediateFuture(SessionResult(SessionResult.RESULT_ERROR_NOT_SUPPORTED))
            }
            session.player.stop()
            session.player.clearMediaItems()
            stopSelf()
            return Futures.immediateFuture(SessionResult(SessionResult.RESULT_SUCCESS))
        }
    }

    @androidx.annotation.OptIn(markerClass = [UnstableApi::class])
    override fun onCreate() {
        super.onCreate()
        val stopButton = CommandButton.Builder(CommandButton.ICON_STOP)
            .setDisplayName(STOP_LABEL)
            .setSessionCommand(stopCommand)
            .build()
        session = MediaSession.Builder(this, StreamPlayerHolder.obtain(this))
            .setId(SESSION_ID)
            .setCallback(callback)
            .setMediaButtonPreferences(ImmutableList.of(stopButton))
            .build()
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? = session

    override fun onTaskRemoved(rootIntent: Intent?) {
        val player = session?.player
        if (player == null || !player.playWhenReady || player.mediaItemCount == 0) stopSelf()
    }

    override fun onDestroy() {
        session?.release()
        session = null
        super.onDestroy()
    }

    private companion object {
        const val ACTION_STOP = "com.bridgething.companion.STREAM_STOP"
        const val SESSION_ID = "bridgething-stream"
        const val STOP_LABEL = "Stop"
    }
}
