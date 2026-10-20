package com.bridgething.companion.shell

import android.content.ComponentName
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.MediaMetadata
import android.media.Rating
import android.media.session.MediaController
import android.media.session.MediaSession
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.net.Uri
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.support.v4.media.session.MediaControllerCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import androidx.core.app.NotificationManagerCompat
import java.security.MessageDigest
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.CoroutineName
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import uniffi.bridgething_companion.MediaArt
import uniffi.bridgething_companion.MediaArtSink
import uniffi.bridgething_companion.MediaControl
import uniffi.bridgething_companion.MediaQueueEntry
import uniffi.bridgething_companion.MediaRepeatMode
import uniffi.bridgething_companion.MediaSessionBackend
import uniffi.bridgething_companion.MediaSessionInbox
import uniffi.bridgething_companion.MediaSessionSnapshot
import uniffi.bridgething_companion.MediaSnapshotSink

public class AndroidMediaSessionBackend(
    context: Context,
    private val notificationListener: ComponentName,
) : MediaSessionBackend {
    private val appContext = context.applicationContext
    private val manager = appContext.getSystemService(Context.MEDIA_SESSION_SERVICE) as MediaSessionManager
    private val compatByToken = ConcurrentHashMap<MediaSession.Token, MediaControllerCompat>()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO + CoroutineName("bridgething-media-art"))

    private val lock = Any()
    private var thread: HandlerThread? = null
    private var handler: Handler? = null
    private var setListener: MediaSessionManager.OnActiveSessionsChangedListener? = null
    private var heldInbox: MediaSessionInbox? = null
    private val perController = mutableListOf<Pair<MediaController, MediaController.Callback>>()
    private val perCompat = mutableListOf<Pair<MediaControllerCompat, MediaControllerCompat.Callback>>()

    override fun isAccessGranted(): Boolean =
        NotificationManagerCompat.getEnabledListenerPackages(appContext).contains(appContext.packageName)

    public fun refresh() {
        val inbox = synchronized(lock) { heldInbox } ?: return
        start(inbox)
        runCatching { inbox.onSessionsChanged() }
    }

    override fun start(inbox: MediaSessionInbox) {
        detachListeners()
        val handler = ensureHandler()
        val previous = synchronized(lock) {
            val held = heldInbox
            heldInbox = inbox
            held
        }
        if (previous !== inbox) previous?.close()
        val onChanged = { runCatching { inbox.onSessionsChanged() }; Unit }
        val listener = MediaSessionManager.OnActiveSessionsChangedListener { controllers ->
            attachPerController(controllers ?: emptyList(), handler, onChanged)
            onChanged()
        }
        val registered = runCatching {
            manager.addOnActiveSessionsChangedListener(listener, notificationListener, handler)
        }.isSuccess
        if (registered) {
            synchronized(lock) { setListener = listener }
            attachPerController(activeControllers(), handler, onChanged)
        }
    }

    override fun stop() {
        detachListeners()
        synchronized(lock) {
            thread?.quitSafely()
            thread = null
            handler = null
        }
    }

    private fun ensureHandler(): Handler = synchronized(lock) {
        val live = thread?.takeIf { it.isAlive }
        if (live == null) {
            val started = HandlerThread("bridgething-media-sessions").apply { start() }
            thread = started
            handler = Handler(started.looper)
        }
        checkNotNull(handler)
    }

    private fun detachListeners() {
        val listener = synchronized(lock) {
            val held = setListener
            setListener = null
            held
        }
        if (listener != null) runCatching { manager.removeOnActiveSessionsChangedListener(listener) }
        synchronized(lock) {
            perController.forEach { (c, cb) -> runCatching { c.unregisterCallback(cb) } }
            perController.clear()
            perCompat.forEach { (c, cb) -> runCatching { c.unregisterCallback(cb) } }
            perCompat.clear()
        }
    }

    override fun snapshotAll(sink: MediaSnapshotSink) {
        scope.launch {
            sink.use { held -> held.complete(activeControllers().mapNotNull { snapshot(it, compatFor(it)) }) }
        }
    }

    override fun control(`package`: String, cmd: MediaControl) {
        val controller = controllerFor(`package`) ?: return
        val compat = compatFor(controller)
        when (cmd) {
            is MediaControl.Play -> runCatching { controller.transportControls.play() }
            is MediaControl.Pause -> runCatching { controller.transportControls.pause() }
            is MediaControl.SkipNext -> runCatching { controller.transportControls.skipToNext() }
            is MediaControl.SkipPrev -> runCatching { controller.transportControls.skipToPrevious() }
            is MediaControl.SeekTo -> runCatching { controller.transportControls.seekTo(cmd.positionMs) }
            is MediaControl.SkipToQueueItem -> runCatching { controller.transportControls.skipToQueueItem(cmd.queueId) }
            is MediaControl.SetShuffle -> runCatching {
                compat?.transportControls?.setShuffleMode(
                    if (cmd.on) PlaybackStateCompat.SHUFFLE_MODE_ALL else PlaybackStateCompat.SHUFFLE_MODE_NONE,
                )
            }
            is MediaControl.SetRepeat -> runCatching {
                compat?.transportControls?.setRepeatMode(
                    when (cmd.mode) {
                        MediaRepeatMode.OFF -> PlaybackStateCompat.REPEAT_MODE_NONE
                        MediaRepeatMode.ONE -> PlaybackStateCompat.REPEAT_MODE_ONE
                        MediaRepeatMode.ALL -> PlaybackStateCompat.REPEAT_MODE_ALL
                    },
                )
            }
            is MediaControl.SetSpeed -> runCatching { compat?.transportControls?.setPlaybackSpeed(cmd.speed) }
            is MediaControl.SetLiked -> {
                val rating = when (runCatching { controller.ratingType }.getOrDefault(Rating.RATING_NONE)) {
                    Rating.RATING_HEART -> Rating.newHeartRating(cmd.liked)
                    Rating.RATING_THUMB_UP_DOWN -> Rating.newThumbRating(cmd.liked)
                    else -> return
                }
                runCatching { controller.transportControls.setRating(rating) }
            }
        }
    }

    override fun art(`package`: String, token: String, sink: MediaArtSink) {
        val controller = controllerFor(`package`)
        if (controller == null) {
            sink.use { it.complete(null) }
            return
        }
        scope.launch { sink.use { it.complete(resolveArt(controller, token)) } }
    }

    private fun activeControllers(): List<MediaController> =
        runCatching { manager.getActiveSessions(notificationListener) }.getOrDefault(emptyList())

    private fun controllerFor(packageName: String): MediaController? =
        activeControllers().firstOrNull { it.packageName == packageName }

    private fun compatFor(controller: MediaController): MediaControllerCompat? =
        runCatching {
            compatByToken.getOrPut(controller.sessionToken) {
                MediaControllerCompat(appContext, MediaSessionCompat.Token.fromToken(controller.sessionToken))
            }
        }.getOrNull()

    private fun attachPerController(
        controllers: List<MediaController>,
        handler: Handler,
        onChanged: () -> Unit,
    ) = synchronized(lock) {
        perController.forEach { (c, cb) -> runCatching { c.unregisterCallback(cb) } }
        perController.clear()
        perCompat.forEach { (c, cb) -> runCatching { c.unregisterCallback(cb) } }
        perCompat.clear()
        compatByToken.keys.retainAll(controllers.map { it.sessionToken }.toSet())
        for (c in controllers) {
            val cb = object : MediaController.Callback() {
                override fun onPlaybackStateChanged(state: PlaybackState?) = onChanged()

                override fun onMetadataChanged(metadata: MediaMetadata?) = onChanged()

                override fun onQueueChanged(queue: MutableList<MediaSession.QueueItem>?) = onChanged()

                override fun onSessionDestroyed() = onChanged()
            }
            runCatching { c.registerCallback(cb, handler) }
            perController.add(c to cb)
            val compat = compatFor(c) ?: continue
            val ccb = object : MediaControllerCompat.Callback() {
                override fun onShuffleModeChanged(shuffleMode: Int) = onChanged()

                override fun onRepeatModeChanged(repeatMode: Int) = onChanged()

                override fun onSessionReady() = onChanged()
            }
            runCatching { compat.registerCallback(ccb, handler) }
            perCompat.add(compat to ccb)
        }
    }

    private fun snapshot(controller: MediaController, compat: MediaControllerCompat?): MediaSessionSnapshot? {
        val metadata = controller.metadata
        val playback = controller.playbackState
        val title = metadata?.getText(MediaMetadata.METADATA_KEY_TITLE)?.toString()?.takeIf { it.isNotEmpty() }
        val artist = (
            metadata?.getText(MediaMetadata.METADATA_KEY_ARTIST)
                ?: metadata?.getText(MediaMetadata.METADATA_KEY_ALBUM_ARTIST)
            )?.toString()?.takeIf { it.isNotEmpty() }
        val album = metadata?.getText(MediaMetadata.METADATA_KEY_ALBUM)?.toString()?.takeIf { it.isNotEmpty() }
        if (title == null && artist == null && album == null) return null
        val durationMs = metadata?.getLong(MediaMetadata.METADATA_KEY_DURATION)?.takeIf { it > 0L }
        val actions = playback?.actions ?: 0L
        val activeId = playback?.activeQueueItemId?.takeIf { it != MediaSession.QueueItem.UNKNOWN_ID.toLong() }
        val playing = playback?.state == PlaybackState.STATE_PLAYING || playback?.state == PlaybackState.STATE_BUFFERING
        val positionAgeMs = playback?.lastPositionUpdateTime?.takeIf { playing && it > 0L }
            ?.let { (SystemClock.elapsedRealtime() - it).coerceAtLeast(0L) }
        val (liked, likeSupported) = readRating(controller, metadata, actions)
        return MediaSessionSnapshot(
            `package` = controller.packageName,
            title = title,
            artist = artist,
            album = album,
            durationMs = durationMs,
            positionMs = playback?.position ?: 0L,
            playing = playing,
            canSeek = (actions and PlaybackState.ACTION_SEEK_TO) != 0L,
            artToken = metadata?.let { nowPlayingArtToken(it) },
            queue = readQueue(controller),
            activeQueueId = activeId,
            shuffle = readShuffle(compat),
            repeat = readRepeat(compat),
            speed = playback?.playbackSpeed?.takeIf { playing && it > 0f },
            positionAgeMs = positionAgeMs,
            liked = liked,
            likeSupported = likeSupported,
            queueTitle = runCatching { controller.queueTitle }.getOrNull()?.toString()?.takeIf { it.isNotEmpty() },
        )
    }

    private fun readShuffle(compat: MediaControllerCompat?): Boolean? =
        when (runCatching { compat?.shuffleMode }.getOrNull()) {
            PlaybackStateCompat.SHUFFLE_MODE_NONE -> false
            PlaybackStateCompat.SHUFFLE_MODE_ALL, PlaybackStateCompat.SHUFFLE_MODE_GROUP -> true
            else -> null
        }

    private fun readRepeat(compat: MediaControllerCompat?): MediaRepeatMode? =
        when (runCatching { compat?.repeatMode }.getOrNull()) {
            PlaybackStateCompat.REPEAT_MODE_NONE -> MediaRepeatMode.OFF
            PlaybackStateCompat.REPEAT_MODE_ONE -> MediaRepeatMode.ONE
            PlaybackStateCompat.REPEAT_MODE_ALL, PlaybackStateCompat.REPEAT_MODE_GROUP -> MediaRepeatMode.ALL
            else -> null
        }

    private fun readRating(
        controller: MediaController,
        metadata: MediaMetadata?,
        actions: Long,
    ): Pair<Boolean?, Boolean> {
        val type = runCatching { controller.ratingType }.getOrDefault(Rating.RATING_NONE)
        if (type != Rating.RATING_HEART && type != Rating.RATING_THUMB_UP_DOWN) return null to false
        val canSet = (actions and PlaybackStateCompat.ACTION_SET_RATING) != 0L
        val rating = metadata?.getRating(MediaMetadata.METADATA_KEY_USER_RATING)
        val liked = when {
            rating == null || !rating.isRated -> false
            type == Rating.RATING_HEART -> rating.hasHeart()
            else -> rating.isThumbUp
        }
        return liked to canSet
    }

    private fun readQueue(controller: MediaController): List<MediaQueueEntry> =
        runCatching { controller.queue }.getOrNull().orEmpty().map { item ->
            val d = item.description
            MediaQueueEntry(
                queueId = item.queueId,
                title = d.title?.toString()?.takeIf { it.isNotEmpty() },
                subtitle = d.subtitle?.toString()?.takeIf { it.isNotEmpty() },
                artToken = queueArtToken(item),
            )
        }

    private fun resolveArt(controller: MediaController, token: String): MediaArt? {
        controller.metadata?.let { m ->
            if (nowPlayingArtToken(m) == token) {
                nowPlayingArtUri(m)?.let { return encodeUri(it) }
                nowPlayingArtBitmap(m)?.let { return encodeBitmap(it) }
            }
        }
        val item = runCatching { controller.queue }.getOrNull().orEmpty().firstOrNull { queueArtToken(it) == token }
            ?: return null
        item.description.iconUri?.let { return encodeUri(it) }
        item.description.iconBitmap?.let { return encodeBitmap(it) }
        return null
    }

    private fun nowPlayingArtToken(m: MediaMetadata): String? {
        nowPlayingArtUri(m)?.let { return "u${digest(it.toString())}" }
        val bitmap = nowPlayingArtBitmap(m) ?: return null
        val title = m.getText(MediaMetadata.METADATA_KEY_TITLE) ?: ""
        val album = m.getText(MediaMetadata.METADATA_KEY_ALBUM) ?: ""
        val artist = m.getText(MediaMetadata.METADATA_KEY_ARTIST) ?: ""
        return "b${digest("$title|$album|$artist|${bitmap.width}x${bitmap.height}")}"
    }

    private fun queueArtToken(item: MediaSession.QueueItem): String? {
        item.description.iconUri?.let { return "u${digest(it.toString())}" }
        val bitmap = item.description.iconBitmap ?: return null
        return "b${digest("${item.queueId}|${item.description.title ?: ""}|${bitmap.width}x${bitmap.height}")}"
    }

    private fun nowPlayingArtBitmap(m: MediaMetadata): Bitmap? =
        m.getBitmap(MediaMetadata.METADATA_KEY_ART)
            ?: m.getBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART)
            ?: m.getBitmap(MediaMetadata.METADATA_KEY_DISPLAY_ICON)

    private fun nowPlayingArtUri(m: MediaMetadata): Uri? =
        listOf(
            MediaMetadata.METADATA_KEY_ART_URI,
            MediaMetadata.METADATA_KEY_ALBUM_ART_URI,
            MediaMetadata.METADATA_KEY_DISPLAY_ICON_URI,
        ).firstNotNullOfOrNull { key ->
            m.getString(key)?.takeIf { it.isNotEmpty() }?.let(Uri::parse)?.takeIf { isLocalUri(it) }
        }

    private fun isLocalUri(uri: Uri): Boolean =
        uri.scheme in setOf("content", "file", "android.resource")

    private fun encodeUri(uri: Uri): MediaArt? = runCatching {
        val bitmap = appContext.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it) }
            ?: return null
        encodeBitmap(bitmap)
    }.getOrNull()

    private fun encodeBitmap(bitmap: Bitmap): MediaArt? =
        scaleToJpeg(bitmap, MAX_ART_EDGE, ART_JPEG_QUALITY)?.let { MediaArt(bytes = it.bytes, mime = it.mime) }

    private fun digest(value: String): String =
        MessageDigest.getInstance("MD5").digest(value.toByteArray()).joinToString("") { "%02x".format(it) }

    private companion object {
        const val MAX_ART_EDGE = 512
        const val ART_JPEG_QUALITY = 0.6f
    }
}
