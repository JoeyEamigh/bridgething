package com.bridgething.companion

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import android.media.Image
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR
import android.media.MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_VBR
import android.media.MediaExtractor
import android.media.MediaFormat
import android.media.MediaMetadataRetriever
import android.media.MediaMuxer
import androidx.media3.common.util.UnstableApi
import androidx.media3.transformer.Codec
import androidx.media3.transformer.DefaultEncoderFactory
import androidx.media3.transformer.VideoEncoderSettings
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.bridgething.companion.shell.AndroidImageScaler
import com.bridgething.companion.shell.AndroidMediaTranscoder
import com.bridgething.companion.shell.avcBitrateModes
import java.io.File
import java.nio.ByteBuffer
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.math.abs
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.bridgething_companion.ImageTarget
import uniffi.bridgething_companion.MediaKind
import uniffi.bridgething_companion.MediaSniff
import uniffi.bridgething_companion.NoHandle
import uniffi.bridgething_companion.TranscodeRequest
import uniffi.bridgething_companion.TranscodeSink
import uniffi.bridgething_companion.TranscodedVideo
import uniffi.bridgething_companion.planVideo

private class RecordingTranscodeSink : TranscodeSink(NoHandle) {
    val settled = LinkedBlockingQueue<Result<TranscodedVideo>>()

    override fun complete(video: TranscodedVideo) {
        settled.add(Result.success(video))
    }

    override fun fail(reason: String) {
        settled.add(Result.failure(IllegalStateException(reason)))
    }
}

private class Scene(val width: Int, val height: Int, val rotation: Int) {
    val side: Int = minOf(width / 2, height) / 3 / 4 * 4
    val displayWidth: Int = if (rotation % 180 == 0) width else height
    val displayHeight: Int = if (rotation % 180 == 0) height else width

    fun colorAt(x: Int, y: Int): Int = when {
        x in side / 2 until side * 3 / 2 && y in side / 2 until side * 3 / 2 -> Color.WHITE
        x < width / 2 -> Color.RED
        else -> Color.BLUE
    }

    fun upright(x: Int, y: Int): Pair<Int, Int> = when (rotation) {
        90 -> (height - 1 - y) to x
        180 -> (width - 1 - x) to (height - 1 - y)
        270 -> y to (width - 1 - x)
        else -> x to y
    }

    fun planes(): List<Plane> = List(3) { plane ->
        val step = if (plane == 0) 1 else 2
        val across = width / step
        val down = height / step
        Plane(across, down, ByteArray(across * down) { yuv(colorAt(it % across * step, it / across * step))[plane] })
    }

    private fun yuv(color: Int): ByteArray = when (color) {
        Color.WHITE -> WHITE_YUV
        Color.RED -> RED_YUV
        else -> BLUE_YUV
    }

    private companion object {
        val WHITE_YUV = byteArrayOf(235.toByte(), 128.toByte(), 128.toByte())
        val RED_YUV = byteArrayOf(81, 90, 240.toByte())
        val BLUE_YUV = byteArrayOf(41, 240.toByte(), 110)
    }
}

private class Plane(val width: Int, val height: Int, val samples: ByteArray) {
    fun copyInto(target: Image.Plane, shift: Int = 0) {
        val buffer = target.buffer
        val offset = Math.floorMod(shift, width)
        for (row in 0 until height) {
            val start = row * target.rowStride
            if (target.pixelStride == 1 && offset == 0) {
                buffer.position(start)
                buffer.put(samples, row * width, width)
            } else {
                for (column in 0 until width) {
                    val source = row * width + (column + offset) % width
                    buffer.put(start + column * target.pixelStride, samples[source])
                }
            }
        }
    }
}

private class Sample(val data: ByteArray, val info: MediaCodec.BufferInfo)

private class Encoded(val format: MediaFormat, val samples: List<Sample>)

@RunWith(AndroidJUnit4::class)
class AndroidMediaTranscoderTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext

    private companion object {
        val CEILING = 300_000u
    }

    @Test
    fun aRotated60FpsClipWithAudioBecomesAnUprightSilent30FpsH264AtThePlannedSize() {
        val scene = Scene(width = 1600, height = 900, rotation = 90)
        val source = File(context.cacheDir, "transcode-source.mp4")
        val destination = File(context.cacheDir, "transcode-output.mp4")
        writeSource(source, scene, fps = 60, seconds = 2)
        assertEquals(2, trackCount(source))

        val sink = RecordingTranscodeSink()
        AndroidMediaTranscoder(context).transcode(TranscodeRequest(source.path, destination.path, 800u, 480u, null), sink)
        assertTrue(sink.settled.isEmpty())
        val video = checkNotNull(sink.settled.poll(180, TimeUnit.SECONDS)).getOrThrow()
        assertTrue(sink.settled.isEmpty())

        val plan = planVideo(900u, 1600u, 800u, 480u, null)
        assertEquals(plan.width to plan.height, video.width to video.height)

        val extractor = MediaExtractor().apply { setDataSource(destination.path) }
        assertEquals(1, extractor.trackCount)
        val format = extractor.getTrackFormat(0)
        assertEquals(MediaFormat.MIMETYPE_VIDEO_AVC, format.getString(MediaFormat.KEY_MIME))
        assertEquals(plan.width.toInt(), format.getInteger(MediaFormat.KEY_WIDTH))
        assertEquals(plan.height.toInt(), format.getInteger(MediaFormat.KEY_HEIGHT))
        assertFalse(format.containsKey(MediaFormat.KEY_ROTATION) && format.getInteger(MediaFormat.KEY_ROTATION) != 0)
        extractor.selectTrack(0)
        val timestamps = generateSequence {
            extractor.sampleTime.takeIf { it >= 0 }?.also { extractor.advance() }
        }.toList().sorted()
        extractor.release()
        val fps = (timestamps.size - 1) * 1_000_000.0 / (timestamps.last() - timestamps.first())
        assertTrue("fps $fps", fps <= 30.5)
        assertTrue("frames ${timestamps.size}", timestamps.size >= 50)

        val retriever = MediaMetadataRetriever().apply { setDataSource(destination.path) }
        assertEquals("0", retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION))
        assertEquals(null, retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_HAS_AUDIO))
        val duration = checkNotNull(retriever.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)).toLong()
        val frame = checkNotNull(retriever.getFrameAtTime(0))
        retriever.release()
        assertEquals(duration.toULong(), video.durationMs)
        assertTrue("duration $duration", duration in 1800L..2100L)
        assertEquals(plan.width.toInt() to plan.height.toInt(), frame.width to frame.height)
        assertUpright(scene, frame)

        source.delete()
        destination.delete()
    }

    @Test
    fun aSourceThatCannotBeReadFailsAndLeavesNoDestination() {
        val destination = File(context.cacheDir, "transcode-stale.mp4").apply { writeText("stale") }
        val sink = RecordingTranscodeSink()
        AndroidMediaTranscoder(context).transcode(
            TranscodeRequest(File(context.cacheDir, "missing.mp4").path, destination.path, 800u, 480u, null),
            sink,
        )
        val outcome = checkNotNull(sink.settled.poll(60, TimeUnit.SECONDS))
        assertTrue(outcome.isFailure)
        assertFalse(destination.exists())
    }

    @Test
    fun cancellingAnInFlightTranscodeSettlesItAsAFailureAndLeavesNothing() {
        val scene = Scene(width = 1600, height = 900, rotation = 0)
        val source = File(context.cacheDir, "transcode-long.mp4")
        val destination = File(context.cacheDir, "transcode-cancelled.mp4")
        writeSource(source, scene, fps = 30, seconds = 8)

        val sink = RecordingTranscodeSink()
        val job = AndroidMediaTranscoder(context).transcode(
            TranscodeRequest(source.path, destination.path, 800u, 480u, null),
            sink,
        )
        job.cancel()
        val outcome = checkNotNull(sink.settled.poll(60, TimeUnit.SECONDS))
        source.delete()

        assertTrue("a cancelled transcode must not complete, got $outcome", outcome.isFailure)
        assertFalse(destination.exists())
        assertNull(sink.settled.poll(2, TimeUnit.SECONDS))
    }

    @androidx.annotation.OptIn(markerClass = [UnstableApi::class])
    private fun askedFor(transcoder: (factory: (Context, VideoEncoderSettings) -> Codec.EncoderFactory) -> AndroidMediaTranscoder): List<VideoEncoderSettings> {
        val scene = Scene(width = 960, height = 540, rotation = 0)
        val source = File(context.cacheDir, "transcode-drifting.mp4")
        writeSource(source, scene, fps = 30, seconds = 2, drift = 6)
        val asked = LinkedBlockingQueue<VideoEncoderSettings>()
        val encoding = transcoder { appContext, settings ->
            asked.add(settings)
            DefaultEncoderFactory.Builder(appContext).setRequestedVideoEncoderSettings(settings).build()
        }
        for (maxBitrate in listOf(null, CEILING)) {
            val destination = File(context.cacheDir, "transcode-drifting-${maxBitrate ?: 0u}.mp4")
            val sink = RecordingTranscodeSink()
            encoding.transcode(TranscodeRequest(source.path, destination.path, 800u, 480u, maxBitrate), sink)
            checkNotNull(sink.settled.poll(180, TimeUnit.SECONDS)).getOrThrow()
            assertTrue(destination.length() > 0)
            destination.delete()
        }
        source.delete()
        return listOf(checkNotNull(asked.poll()), checkNotNull(asked.poll()))
    }

    @Test
    @androidx.annotation.OptIn(markerClass = [UnstableApi::class])
    fun aCeilingReachesAnEncoderThatHoldsConstantBitrateAsOneSoNoPlatformFloorRaisesIt() {
        val quality = planVideo(960u, 540u, 800u, 480u, null).bitrate
        assertTrue(quality > CEILING * 3u)

        val (free, capped) = askedFor { factory -> AndroidMediaTranscoder(context, { listOf({ true }) }, factory) }

        assertEquals(quality.toInt() to BITRATE_MODE_VBR, free.bitrate to free.bitrateMode)
        assertEquals(CEILING.toInt() to BITRATE_MODE_CBR, capped.bitrate to capped.bitrateMode)
    }

    @Test
    @androidx.annotation.OptIn(markerClass = [UnstableApi::class])
    fun aCeilingOnAPhoneWithNoConstantBitrateEncoderStillExportsAsVariableBitrate() {
        val variableOnly: (Int) -> Boolean = { mode -> mode == BITRATE_MODE_VBR }

        val (_, capped) = askedFor { factory -> AndroidMediaTranscoder(context, { listOf(variableOnly) }, factory) }

        assertEquals(CEILING.toInt() to BITRATE_MODE_VBR, capped.bitrate to capped.bitrateMode)
    }

    @Test
    fun thisPhoneIsAskedForConstantBitrateExactlyWhenOneOfItsEncodersHoldsIt() {
        val holds = avcBitrateModes().any { supports -> supports(BITRATE_MODE_CBR) }

        val (_, capped) = askedFor { factory -> AndroidMediaTranscoder(context, ::avcBitrateModes, factory) }

        assertEquals(if (holds) BITRATE_MODE_CBR else BITRATE_MODE_VBR, capped.bitrateMode)
    }

    @Test
    fun aPosterIsTheFirstFrameAsItPlaysWhateverRotationTheSourceCarries() {
        for ((width, height) in listOf(1600 to 900, 720 to 720)) {
            for (rotation in listOf(0, 90, 180, 270)) {
                val scene = Scene(width, height, rotation)
                val source = File(context.cacheDir, "poster-source.mp4")
                writeSource(source, scene, fps = 30, seconds = 1)

                val poster = checkNotNull(AndroidMediaTranscoder(context).poster(source.path))
                source.delete()

                assertEquals(0xFF.toByte() to 0xD8.toByte(), poster[0] to poster[1])
                val frame = checkNotNull(BitmapFactory.decodeByteArray(poster, 0, poster.size))
                val label = "${width}x$height tagged $rotation"
                assertEquals(label, scene.displayWidth to scene.displayHeight, frame.width to frame.height)
                assertUpright(scene, frame)

                val posterSniff = MediaSniff(uniffi.bridgething_companion.MediaFormat.JPEG, MediaKind.IMAGE, false, "image/jpeg")
                val thumbnail = checkNotNull(AndroidImageScaler().scale(poster, posterSniff, ImageTarget.Thumbnail(320u, 0.7f)))
                assertEquals("image/jpeg", thumbnail.mime)
                assertEquals(label, 320u, maxOf(thumbnail.width, thumbnail.height))
                val preview = BitmapFactory.decodeByteArray(thumbnail.bytes, 0, thumbnail.bytes.size)
                assertUpright(scene, checkNotNull(preview))
            }
        }
    }

    @Test
    fun aPosterOfSomethingThatIsNotAVideoIsNull() {
        val text = File(context.cacheDir, "poster-notes.txt").apply { writeText("plain words") }
        assertNull(AndroidMediaTranscoder(context).poster(text.path))
        assertNull(AndroidMediaTranscoder(context).poster(File(context.cacheDir, "missing.mp4").path))
        text.delete()
    }

    private fun trackCount(file: File): Int =
        MediaExtractor().run {
            setDataSource(file.path)
            trackCount.also { release() }
        }

    private fun assertUpright(scene: Scene, frame: Bitmap) {
        val label = "${scene.width}x${scene.height} tagged ${scene.rotation} as ${frame.width}x${frame.height}"
        fun at(x: Int, y: Int): Pair<Int, Int> {
            val (uprightX, uprightY) = scene.upright(x, y)
            return uprightX * frame.width / scene.displayWidth to uprightY * frame.height / scene.displayHeight
        }
        val (redX, redY) = at(scene.width / 4, scene.height * 5 / 6)
        assertEquals(label, Color.RED, nearest(frame.getPixel(redX, redY)))
        val (blueX, blueY) = at(scene.width * 3 / 4, scene.height / 2)
        assertEquals(label, Color.BLUE, nearest(frame.getPixel(blueX, blueY)))
        val (markX, markY) = at(scene.side, scene.side)
        assertEquals(label, Color.WHITE, nearest(frame.getPixel(markX, markY)))

        val across = (0 until frame.width).count { nearest(frame.getPixel(it, markY)) == Color.WHITE }
        val down = (0 until frame.height).count { nearest(frame.getPixel(markX, it)) == Color.WHITE }
        assertEquals(label, scene.side * frame.width / scene.displayWidth.toFloat(), across.toFloat(), MARK_SLACK)
        assertEquals(label, scene.side * frame.height / scene.displayHeight.toFloat(), down.toFloat(), MARK_SLACK)
        assertEquals(label, across.toFloat(), down.toFloat(), MARK_SLACK)
    }

    private fun nearest(pixel: Int): Int =
        listOf(Color.RED, Color.BLUE, Color.WHITE).minBy { candidate ->
            abs(Color.red(candidate) - Color.red(pixel)) +
                abs(Color.green(candidate) - Color.green(pixel)) +
                abs(Color.blue(candidate) - Color.blue(pixel))
        }

    private fun writeSource(file: File, scene: Scene, fps: Int, seconds: Int, drift: Int = 0) {
        val video = encodeVideo(scene, fps, seconds, drift)
        val audio = encodeSilence(seconds)
        val muxer = MediaMuxer(file.path, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
        muxer.setOrientationHint(scene.rotation)
        val videoTrack = muxer.addTrack(video.format)
        val audioTrack = muxer.addTrack(audio.format)
        muxer.start()
        video.samples.forEach { muxer.writeSampleData(videoTrack, ByteBuffer.wrap(it.data), it.info) }
        audio.samples.forEach { muxer.writeSampleData(audioTrack, ByteBuffer.wrap(it.data), it.info) }
        muxer.stop()
        muxer.release()
    }

    private fun encodeVideo(scene: Scene, fps: Int, seconds: Int, drift: Int): Encoded {
        val planes = scene.planes()
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, scene.width, scene.height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible)
            setInteger(MediaFormat.KEY_BIT_RATE, 4_000_000)
            setInteger(MediaFormat.KEY_FRAME_RATE, fps)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
        }
        val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        return drive(codec, fps * seconds, { frame -> frame * 1_000_000L / fps }) { input, frame ->
            val image = checkNotNull(codec.getInputImage(input))
            image.planes.forEachIndexed { index, target ->
                planes[index].copyInto(target, frame * drift / if (index == 0) 1 else 2)
            }
            scene.width * scene.height * 3 / 2
        }
    }

    private fun encodeSilence(seconds: Int): Encoded {
        val sampleRate = 44_100
        val samplesPerBuffer = 1024
        val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_AAC, sampleRate, 1).apply {
            setInteger(MediaFormat.KEY_AAC_PROFILE, MediaCodecInfo.CodecProfileLevel.AACObjectLC)
            setInteger(MediaFormat.KEY_BIT_RATE, 64_000)
        }
        val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_AAC)
        codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        val buffers = seconds * sampleRate / samplesPerBuffer
        return drive(codec, buffers, { index -> index * samplesPerBuffer * 1_000_000L / sampleRate }) { input, _ ->
            val buffer = checkNotNull(codec.getInputBuffer(input))
            buffer.clear()
            buffer.put(ByteArray(samplesPerBuffer * 2))
            samplesPerBuffer * 2
        }
    }

    private fun drive(codec: MediaCodec, inputs: Int, pts: (Int) -> Long, fill: (Int, Int) -> Int): Encoded {
        codec.start()
        val samples = mutableListOf<Sample>()
        val info = MediaCodec.BufferInfo()
        var format: MediaFormat? = null
        var fed = 0
        var ended = false
        while (!ended) {
            if (fed <= inputs) {
                val input = codec.dequeueInputBuffer(10_000)
                if (input >= 0) {
                    if (fed == inputs) {
                        codec.queueInputBuffer(input, 0, 0, pts(fed), MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                    } else {
                        codec.queueInputBuffer(input, 0, fill(input, fed), pts(fed), 0)
                    }
                    fed++
                }
            }
            val output = codec.dequeueOutputBuffer(info, 10_000)
            if (output == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) format = codec.outputFormat
            if (output >= 0) {
                if (info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                    val buffer = checkNotNull(codec.getOutputBuffer(output))
                    buffer.position(info.offset)
                    buffer.limit(info.offset + info.size)
                    val data = ByteArray(info.size).also { buffer.get(it) }
                    val flags = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM.inv()
                    samples += Sample(data, MediaCodec.BufferInfo().apply { set(0, data.size, info.presentationTimeUs, flags) })
                }
                ended = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                codec.releaseOutputBuffer(output, false)
            }
        }
        codec.stop()
        codec.release()
        return Encoded(checkNotNull(format), samples)
    }
}

private const val MARK_SLACK = 6f
