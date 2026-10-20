package com.bridgething.companion.shell

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Matrix
import android.media.MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR
import android.media.MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_VBR
import android.media.MediaFormat
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.util.UnstableApi
import androidx.media3.effect.FrameDropEffect
import androidx.media3.effect.Presentation
import androidx.media3.transformer.Codec
import androidx.media3.transformer.Composition
import androidx.media3.transformer.DefaultEncoderFactory
import androidx.media3.transformer.EditedMediaItem
import androidx.media3.transformer.EditedMediaItemSequence
import androidx.media3.transformer.EncoderUtil
import androidx.media3.transformer.Effects
import androidx.media3.transformer.ExportException
import androidx.media3.transformer.ExportResult
import androidx.media3.transformer.Transformer
import androidx.media3.transformer.VideoEncoderSettings
import java.io.ByteArrayOutputStream
import java.io.File
import java.util.concurrent.CancellationException
import uniffi.bridgething_companion.MediaTranscoder
import uniffi.bridgething_companion.TranscodeJob
import uniffi.bridgething_companion.TranscodeRequest
import uniffi.bridgething_companion.TranscodeSink
import uniffi.bridgething_companion.TranscodedVideo
import uniffi.bridgething_companion.VideoPlan
import uniffi.bridgething_companion.planVideo

@androidx.annotation.OptIn(markerClass = [UnstableApi::class])
public class AndroidMediaTranscoder internal constructor(
    context: Context,
    private val bitrateModes: () -> List<(Int) -> Boolean>,
    private val encoders: (Context, VideoEncoderSettings) -> Codec.EncoderFactory,
) : MediaTranscoder {
    public constructor(context: Context) : this(context, ::avcBitrateModes, ::defaultEncoders)

    private val appContext: Context = context.applicationContext

    override fun transcode(request: TranscodeRequest, sink: TranscodeSink): TranscodeJob {
        val flight = Flight(File(request.destination), sink)
        handler.post { flight.start(request) }
        return flight
    }

    override fun poster(source: String): ByteArray? = runCatching {
        retrieve(source) { retriever ->
            val decoded = checkNotNull(retriever.getFrameAtTime(0, MediaMetadataRetriever.OPTION_CLOSEST_SYNC))
            val frame = if (retriever.dropsHdrHalfTurn()) decoded.halfTurned() else decoded
            ByteArrayOutputStream().use { out ->
                check(frame.compress(Bitmap.CompressFormat.JPEG, POSTER_QUALITY, out))
                out.toByteArray()
            }
        }
    }.getOrNull()

    private fun transformer(plan: VideoPlan, listener: Transformer.Listener): Transformer =
        Transformer.Builder(appContext)
            .setLooper(handler.looper)
            .setVideoMimeType(MimeTypes.VIDEO_H264)
            .setPortraitEncodingEnabled(true)
            .setEncoderFactory(
                encoders(
                    appContext,
                    VideoEncoderSettings.Builder()
                        .setBitrate(plan.bitrate.toInt())
                        .setBitrateMode(bitrateMode(plan.fitted, bitrateModes()))
                        .setiFrameIntervalSeconds(plan.keyframeSeconds.toFloat())
                        .build(),
                ),
            )
            .addListener(listener)
            .build()

    private fun composition(source: String, plan: VideoPlan): Composition {
        val item = EditedMediaItem.Builder(MediaItem.fromUri(Uri.fromFile(File(source))))
            .setRemoveAudio(true)
            .setEffects(
                Effects(
                    emptyList(),
                    listOf(
                        FrameDropEffect.createDefaultFrameDropEffect(plan.maxFps.toFloat()),
                        Presentation.createForWidthAndHeight(
                            plan.width.toInt(),
                            plan.height.toInt(),
                            Presentation.LAYOUT_STRETCH_TO_FIT,
                        ),
                    ),
                ),
            )
            .build()
        return Composition.Builder(EditedMediaItemSequence.withVideoFrom(listOf(item)))
            .setHdrMode(Composition.HDR_MODE_TONE_MAP_HDR_TO_SDR_USING_OPEN_GL)
            .build()
    }

    private inner class Flight(
        private val destination: File,
        private val sink: TranscodeSink,
    ) : TranscodeJob, Transformer.Listener {
        private var transformer: Transformer? = null
        private var over = false

        fun start(request: TranscodeRequest) {
            if (over) return
            runCatching {
                destination.delete()
                val (width, height) = displaySize(request.source)
                val plan = planVideo(width, height, request.coverWidth, request.coverHeight, request.maxBitrate)
                transformer(plan, this).also { transformer = it }
                    .start(composition(request.source, plan), destination.path)
            }.onFailure(::fail)
        }

        override fun cancel() {
            handler.post {
                if (over) return@post
                transformer?.cancel()
                fail(CancellationException("the transcode was cancelled"))
            }
        }

        override fun onCompleted(composition: Composition, exportResult: ExportResult) {
            if (over) return
            runCatching { probe(destination.path) }.fold({ video ->
                over = true
                sink.complete(video)
            }, ::fail)
        }

        override fun onError(composition: Composition, exportResult: ExportResult, exportException: ExportException) {
            fail(exportException)
        }

        private fun fail(error: Throwable) {
            if (over) return
            over = true
            destination.delete()
            sink.fail(error.toString())
        }
    }

    private companion object {
        val handler: Handler by lazy { Handler(HandlerThread("bridgething-transcode").apply { start() }.looper) }
    }
}

private const val POSTER_QUALITY = 90

internal fun bitrateMode(fitted: Boolean, encoders: List<(Int) -> Boolean>): Int =
    if (fitted && encoders.any { supports -> supports(BITRATE_MODE_CBR) }) BITRATE_MODE_CBR else BITRATE_MODE_VBR

@androidx.annotation.OptIn(markerClass = [UnstableApi::class])
internal fun avcBitrateModes(): List<(Int) -> Boolean> =
    EncoderUtil.getSupportedEncoders(MimeTypes.VIDEO_H264).map { encoder ->
        { mode: Int -> EncoderUtil.isBitrateModeSupported(encoder, MimeTypes.VIDEO_H264, mode) }
    }

@androidx.annotation.OptIn(markerClass = [UnstableApi::class])
private fun defaultEncoders(context: Context, settings: VideoEncoderSettings): Codec.EncoderFactory =
    DefaultEncoderFactory.Builder(context).setRequestedVideoEncoderSettings(settings).build()

private fun displaySize(path: String): Pair<UInt, UInt> = retrieve(path) { retriever ->
    val width = retriever.metadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH).toUInt()
    val height = retriever.metadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT).toUInt()
    val rotation = retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION)?.toLongOrNull() ?: 0L
    if (rotation % 180 == 0L) width to height else height to width
}

private fun probe(path: String): TranscodedVideo = retrieve(path) { retriever ->
    TranscodedVideo(
        width = retriever.metadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH).toUInt(),
        height = retriever.metadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT).toUInt(),
        durationMs = retriever.metadata(MediaMetadataRetriever.METADATA_KEY_DURATION).toULong(),
    )
}

private fun <T> retrieve(path: String, read: (MediaMetadataRetriever) -> T): T {
    val retriever = MediaMetadataRetriever()
    try {
        retriever.setDataSource(path)
        return read(retriever)
    } finally {
        retriever.release()
    }
}

private fun MediaMetadataRetriever.dropsHdrHalfTurn(): Boolean =
    Build.VERSION.SDK_INT >= Build.VERSION_CODES.R &&
        Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU &&
        extractMetadata(MediaMetadataRetriever.METADATA_KEY_COLOR_STANDARD)?.toIntOrNull() ==
        MediaFormat.COLOR_STANDARD_BT2020 &&
        extractMetadata(MediaMetadataRetriever.METADATA_KEY_COLOR_TRANSFER)?.toIntOrNull() in HDR_TRANSFERS &&
        extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION)?.toIntOrNull() == 180

private val HDR_TRANSFERS = setOf(MediaFormat.COLOR_TRANSFER_HLG, MediaFormat.COLOR_TRANSFER_ST2084)

private fun Bitmap.halfTurned(): Bitmap =
    Bitmap.createBitmap(this, 0, 0, width, height, Matrix().apply { postRotate(180f) }, false)

private fun MediaMetadataRetriever.metadata(key: Int): Long =
    checkNotNull(extractMetadata(key)?.toLongOrNull()) { "no metadata for key $key" }
