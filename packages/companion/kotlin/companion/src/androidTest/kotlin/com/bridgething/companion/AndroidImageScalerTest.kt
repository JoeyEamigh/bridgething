package com.bridgething.companion

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.media.ExifInterface
import android.os.Build
import androidx.heifwriter.HeifWriter
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.bridgething.companion.shell.AndroidImageScaler
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import uniffi.bridgething_companion.ImageTarget
import uniffi.bridgething_companion.MediaFormat
import uniffi.bridgething_companion.MediaKind
import uniffi.bridgething_companion.MediaSniff
import uniffi.bridgething_companion.ScaledImage
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.zip.CRC32
import java.util.zip.Deflater
import kotlin.math.abs

@RunWith(AndroidJUnit4::class)
class AndroidImageScalerTest {
    private val scaler = AndroidImageScaler()
    private val cover = ImageTarget.Cover(800u, 800u)

    private fun sniffed(format: MediaFormat, mime: String, animated: Boolean = false) =
        MediaSniff(format, MediaKind.IMAGE, animated, mime)

    private val jpegSniff = sniffed(MediaFormat.JPEG, "image/jpeg")
    private val pngSniff = sniffed(MediaFormat.PNG, "image/png")

    @Test
    fun aCoverTrustsTheSniffItIsHandedInsteadOfSniffingAgain() {
        val jpeg = encode(solid(1200, 900, Color.YELLOW), Bitmap.CompressFormat.JPEG)
        val elsewhere = MediaSniff(MediaFormat.UNKNOWN, MediaKind.OTHER, false, null)
        assertEquals(null, scaler.scale(jpeg, elsewhere, cover))
        assertEquals("image/jpeg", checkNotNull(scaler.scale(jpeg, jpegSniff, cover)).mime)
    }

    @Test
    fun anAnimatedGifPassesThroughByteForByteWithItsCanvasSize() {
        val gif = animatedGif(2000, 1000)
        val result = checkNotNull(scaler.scale(gif, sniffed(MediaFormat.GIF, "image/gif", animated = true), cover))
        assertArrayEquals(gif, result.bytes)
        assertEquals("image/gif", result.mime)
        assertEquals(2000u to 1000u, result.width to result.height)
    }

    @Test
    fun anApngPassesThroughByteForByteWithItsCanvasSize() {
        val apng = apng(1800, 1200)
        val result = checkNotNull(scaler.scale(apng, sniffed(MediaFormat.PNG, "image/png", animated = true), cover))
        assertArrayEquals(apng, result.bytes)
        assertEquals("image/png", result.mime)
        assertEquals(1800u to 1200u, result.width to result.height)
    }

    @Test
    fun anAnimatedWebpPassesThroughByteForByteWithItsCanvasSize() {
        val webp = animatedWebp(1600, 1600)
        val result = checkNotNull(scaler.scale(webp, sniffed(MediaFormat.WEBP, "image/webp", animated = true), cover))
        assertArrayEquals(webp, result.bytes)
        assertEquals("image/webp", result.mime)
        assertEquals(1600u to 1600u, result.width to result.height)
    }

    @Test
    fun aTranslucentPngShrinksToCoverTheBoxAndKeepsItsAlpha() {
        val png = encode(solid(2000, 1000, Color.argb(128, 255, 0, 0)), Bitmap.CompressFormat.PNG)
        val result = checkNotNull(scaler.scale(png, pngSniff, cover))
        assertEquals("image/png", result.mime)
        assertEquals(1600u to 800u, result.width to result.height)
        val decoded = decode(result)
        assertTrue(decoded.hasAlpha())
        assertEquals(128f, Color.alpha(decoded.getPixel(800, 400)).toFloat(), 2f)
    }

    @Test
    fun aLargeJpegShrinksUntilItsShortEdgeMeetsTheBox() {
        val jpeg = encode(solid(4032, 3024, Color.GREEN), Bitmap.CompressFormat.JPEG)
        val result = checkNotNull(scaler.scale(jpeg, jpegSniff, cover))
        assertEquals("image/jpeg", result.mime)
        assertEquals(1067u to 800u, result.width to result.height)
        decode(result)
    }

    @Test
    fun aJpegThatAlreadyFitsIsNeverUpscaledAndKeepsItsBytes() {
        val jpeg = encode(solid(640, 480, Color.BLUE), Bitmap.CompressFormat.JPEG)
        val result = checkNotNull(scaler.scale(jpeg, jpegSniff, cover))
        assertArrayEquals(jpeg, result.bytes)
        assertEquals("image/jpeg", result.mime)
        assertEquals(640u to 480u, result.width to result.height)
    }

    @Test
    fun exifOrientationIsAppliedToThePixelsAndStrippedFromTheOutput() {
        val result = checkNotNull(scaler.scale(redLeftBlueRightJpeg(ExifInterface.ORIENTATION_ROTATE_90), jpegSniff, cover))
        assertEquals("image/jpeg", result.mime)
        assertEquals(800u to 1600u, result.width to result.height)
        val upright = decode(result)
        assertEquals(Color.RED, nearest(upright.getPixel(400, 200)))
        assertEquals(Color.BLUE, nearest(upright.getPixel(400, 1400)))
        assertCarriesNoOrientation(result)
    }

    @Test
    fun aHeicLandsAsAJpegEvenWhenItAlreadyFits() {
        assumeTrue(Build.VERSION.SDK_INT >= Build.VERSION_CODES.P)
        val file = File.createTempFile("still", ".heic", context().cacheDir)
        HeifWriter.Builder(file.path, 640, 480, HeifWriter.INPUT_MODE_BITMAP).setQuality(90).build().use { writer ->
            writer.start()
            writer.addBitmap(solid(640, 480, Color.MAGENTA))
            writer.stop(10_000)
        }
        val heic = file.readBytes()
        file.delete()
        val result = checkNotNull(scaler.scale(heic, sniffed(MediaFormat.HEIF, "image/heic"), cover))
        assertEquals("image/jpeg", result.mime)
        assertEquals(640u to 480u, result.width to result.height)
        assertFalse(result.bytes.contentEquals(heic))
        assertEquals(0xFF.toByte() to 0xD8.toByte(), result.bytes[0] to result.bytes[1])
        decode(result)
    }

    @Test
    fun aThumbnailCapsTheLongEdgeAsAJpeg() {
        val png = encode(solid(2000, 1000, Color.YELLOW), Bitmap.CompressFormat.PNG)
        val result = checkNotNull(scaler.scale(png, pngSniff, ImageTarget.Thumbnail(512u, 0.6f)))
        assertEquals("image/jpeg", result.mime)
        assertEquals(512u to 256u, result.width to result.height)
        assertEquals(512 to 256, decode(result).let { it.width to it.height })
    }

    @Test
    fun aThumbnailAppliesExifOrientationToThePixelsAndStripsItFromTheOutput() {
        val turns = mapOf(
            ExifInterface.ORIENTATION_ROTATE_90 to listOf(256 to 512, 128 to 64, 128 to 448),
            ExifInterface.ORIENTATION_ROTATE_180 to listOf(512 to 256, 448 to 128, 64 to 128),
            ExifInterface.ORIENTATION_ROTATE_270 to listOf(256 to 512, 128 to 448, 128 to 64),
        )
        turns.forEach { (stored, expected) ->
            val (size, red, blue) = expected
            val result = checkNotNull(scaler.scale(redLeftBlueRightJpeg(stored), jpegSniff, ImageTarget.Thumbnail(512u, 0.6f)))
            assertEquals("image/jpeg", result.mime)
            assertEquals("orientation $stored", size, result.width.toInt() to result.height.toInt())
            val upright = decode(result)
            assertEquals("orientation $stored", Color.RED, nearest(upright.getPixel(red.first, red.second)))
            assertEquals("orientation $stored", Color.BLUE, nearest(upright.getPixel(blue.first, blue.second)))
            assertCarriesNoOrientation(result)
        }
    }

    @Test
    fun aVideoIsNotAnImage() {
        val mp4 = byteArrayOf(0, 0, 0, 24) + "ftypisom".toByteArray() + ByteArray(12)
        val video = MediaSniff(MediaFormat.ISO_VIDEO, MediaKind.VIDEO, false, "video/mp4")
        assertEquals(null, scaler.scale(mp4, video, cover))
    }

    private fun context() = InstrumentationRegistry.getInstrumentation().targetContext

    private fun assertCarriesNoOrientation(image: ScaledImage) {
        val orientation =
            ExifInterface(ByteArrayInputStream(image.bytes))
                .getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_UNDEFINED)
        assertTrue(orientation in setOf(ExifInterface.ORIENTATION_UNDEFINED, ExifInterface.ORIENTATION_NORMAL))
    }

    private fun redLeftBlueRightJpeg(orientation: Int): ByteArray {
        val stored = Bitmap.createBitmap(2000, 1000, Bitmap.Config.ARGB_8888)
        Canvas(stored).apply {
            drawRect(0f, 0f, 1000f, 1000f, Paint().apply { color = Color.RED })
            drawRect(1000f, 0f, 2000f, 1000f, Paint().apply { color = Color.BLUE })
        }
        val file = File.createTempFile("oriented", ".jpg", context().cacheDir)
        file.writeBytes(encode(stored, Bitmap.CompressFormat.JPEG))
        ExifInterface(file.path).apply {
            setAttribute(ExifInterface.TAG_ORIENTATION, orientation.toString())
            saveAttributes()
        }
        return file.readBytes().also { file.delete() }
    }

    private fun solid(
        width: Int,
        height: Int,
        color: Int,
    ): Bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888).apply { eraseColor(color) }

    private fun encode(
        bitmap: Bitmap,
        format: Bitmap.CompressFormat,
    ): ByteArray = ByteArrayOutputStream().also { bitmap.compress(format, 90, it) }.toByteArray()

    private fun decode(image: ScaledImage): Bitmap {
        val bitmap = checkNotNull(BitmapFactory.decodeByteArray(image.bytes, 0, image.bytes.size))
        assertEquals(image.width.toInt() to image.height.toInt(), bitmap.width to bitmap.height)
        return bitmap
    }

    private fun nearest(pixel: Int): Int =
        listOf(Color.RED, Color.BLUE).minBy { candidate ->
            abs(Color.red(candidate) - Color.red(pixel)) +
                abs(Color.green(candidate) - Color.green(pixel)) +
                abs(Color.blue(candidate) - Color.blue(pixel))
        }

    private fun animatedGif(
        width: Int,
        height: Int,
    ): ByteArray {
        val out = ByteArrayOutputStream()
        out.write("GIF89a".toByteArray())
        out.write(le16(width) + le16(height) + byteArrayOf(0x80.toByte(), 0, 0))
        out.write(byteArrayOf(0, 0, 0, 0xFF.toByte(), 0xFF.toByte(), 0xFF.toByte()))
        out.write(byteArrayOf(0x21, 0xFF.toByte(), 0x0B) + "NETSCAPE2.0".toByteArray() + byteArrayOf(3, 1, 0, 0, 0))
        repeat(2) { frame ->
            out.write(byteArrayOf(0x21, 0xF9.toByte(), 4, 0, 10, 0, 0, 0))
            out.write(byteArrayOf(0x2C) + le16(frame) + le16(0) + le16(1) + le16(1) + byteArrayOf(0))
            out.write(byteArrayOf(2, 2, (0x44 or (frame shl 3)).toByte(), 0x01, 0))
        }
        out.write(0x3B)
        return out.toByteArray()
    }

    private fun apng(
        width: Int,
        height: Int,
    ): ByteArray {
        val out = ByteArrayOutputStream()
        out.write(byteArrayOf(0x89.toByte(), 'P'.code.toByte(), 'N'.code.toByte(), 'G'.code.toByte(), 13, 10, 26, 10))
        out.write(pngChunk("IHDR", be32(width) + be32(height) + byteArrayOf(8, 2, 0, 0, 0)))
        out.write(pngChunk("acTL", be32(2) + be32(0)))
        val frame = deflate(ByteArray((width * 3 + 1) * height))
        out.write(pngChunk("fcTL", fcTL(0, width, height)))
        out.write(pngChunk("IDAT", frame))
        out.write(pngChunk("fcTL", fcTL(1, width, height)))
        out.write(pngChunk("fdAT", be32(2) + frame))
        out.write(pngChunk("IEND", ByteArray(0)))
        return out.toByteArray()
    }

    private fun fcTL(
        sequence: Int,
        width: Int,
        height: Int,
    ): ByteArray = be32(sequence) + be32(width) + be32(height) + be32(0) + be32(0) + byteArrayOf(0, 1, 0, 10, 0, 0)

    private fun pngChunk(
        type: String,
        data: ByteArray,
    ): ByteArray {
        val typed = type.toByteArray() + data
        val crc = CRC32().apply { update(typed) }.value.toInt()
        return be32(data.size) + typed + be32(crc)
    }

    private fun animatedWebp(
        width: Int,
        height: Int,
    ): ByteArray {
        val frames =
            listOf(Color.RED, Color.BLUE).map { color ->
                val still = encode(solid(1, 1, color), Bitmap.CompressFormat.WEBP_LOSSLESS)
                val vp8l = riffChunks(still).first { it.first == "VP8L" }.second
                riffChunk("ANMF", le24(0) + le24(0) + le24(0) + le24(0) + le24(100) + byteArrayOf(0) + riffChunk("VP8L", vp8l))
            }
        val body =
            "WEBP".toByteArray() +
                riffChunk("VP8X", byteArrayOf(0x12, 0, 0, 0) + le24(width - 1) + le24(height - 1)) +
                riffChunk("ANIM", byteArrayOf(0, 0, 0, 0, 0, 0)) +
                frames.reduce(ByteArray::plus)
        return "RIFF".toByteArray() + le32(body.size) + body
    }

    private fun riffChunks(webp: ByteArray): List<Pair<String, ByteArray>> {
        val chunks = mutableListOf<Pair<String, ByteArray>>()
        var at = 12
        while (at + 8 <= webp.size) {
            val size = ByteBuffer.wrap(webp, at + 4, 4).order(ByteOrder.LITTLE_ENDIAN).int
            chunks += String(webp, at, 4) to webp.copyOfRange(at + 8, at + 8 + size)
            at += 8 + size + (size and 1)
        }
        return chunks
    }

    private fun riffChunk(
        type: String,
        data: ByteArray,
    ): ByteArray = type.toByteArray() + le32(data.size) + data + ByteArray(data.size and 1)

    private fun deflate(raw: ByteArray): ByteArray {
        val deflater =
            Deflater().apply {
                setInput(raw)
                finish()
            }
        val out = ByteArrayOutputStream()
        val buffer = ByteArray(64 * 1024)
        while (!deflater.finished()) out.write(buffer, 0, deflater.deflate(buffer))
        deflater.end()
        return out.toByteArray()
    }

    private fun le16(value: Int) = byteArrayOf(value.toByte(), (value shr 8).toByte())

    private fun le24(value: Int) = byteArrayOf(value.toByte(), (value shr 8).toByte(), (value shr 16).toByte())

    private fun le32(value: Int) =
        ByteBuffer
            .allocate(4)
            .order(ByteOrder.LITTLE_ENDIAN)
            .putInt(value)
            .array()

    private fun be32(value: Int) =
        ByteBuffer
            .allocate(4)
            .order(ByteOrder.BIG_ENDIAN)
            .putInt(value)
            .array()
}
