package com.bridgething.companion.shell

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.graphics.Matrix
import android.media.ExifInterface
import android.os.Build
import androidx.annotation.RequiresApi
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import kotlin.math.roundToInt
import uniffi.bridgething_companion.CoverPlan
import uniffi.bridgething_companion.ImageScaler
import uniffi.bridgething_companion.ImageTarget
import uniffi.bridgething_companion.MediaFormat
import uniffi.bridgething_companion.MediaKind
import uniffi.bridgething_companion.MediaSniff
import uniffi.bridgething_companion.ScaledImage
import uniffi.bridgething_companion.StillFormat
import uniffi.bridgething_companion.planCover

internal fun scaleToJpeg(bitmap: Bitmap, maxEdge: Int, quality: Float): ScaledImage? = runCatching {
    val edge = maxOf(bitmap.width, bitmap.height)
    val scaled = if (edge <= maxEdge || maxEdge <= 0) {
        bitmap
    } else {
        val scale = maxEdge.toFloat() / edge
        Bitmap.createScaledBitmap(
            bitmap,
            (bitmap.width * scale).toInt().coerceAtLeast(1),
            (bitmap.height * scale).toInt().coerceAtLeast(1),
            true,
        )
    }
    encode(scaled, StillFormat.JPEG, "image/jpeg", quality)
}.getOrNull()

internal data class ExifTransform(val degrees: Float, val mirrored: Boolean)

internal fun exifTransform(orientation: Int): ExifTransform = when (orientation) {
    ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> ExifTransform(0f, true)
    ExifInterface.ORIENTATION_ROTATE_180 -> ExifTransform(180f, false)
    ExifInterface.ORIENTATION_FLIP_VERTICAL -> ExifTransform(180f, true)
    ExifInterface.ORIENTATION_TRANSPOSE -> ExifTransform(90f, true)
    ExifInterface.ORIENTATION_ROTATE_90 -> ExifTransform(90f, false)
    ExifInterface.ORIENTATION_TRANSVERSE -> ExifTransform(270f, true)
    ExifInterface.ORIENTATION_ROTATE_270 -> ExifTransform(270f, false)
    else -> ExifTransform(0f, false)
}

public class AndroidImageScaler : ImageScaler {
    override fun scale(bytes: ByteArray, sniff: MediaSniff, target: ImageTarget): ScaledImage? = runCatching {
        when (target) {
            is ImageTarget.Thumbnail -> decodeUpright(bytes, sniff.format)
                ?.let { scaleToJpeg(it, target.maxEdge.toInt(), target.quality) }
            is ImageTarget.Cover -> cover(bytes, sniff, target.width, target.height)
        }
    }.getOrNull()

    private fun cover(bytes: ByteArray, sniff: MediaSniff, coverWidth: UInt, coverHeight: UInt): ScaledImage? {
        if (sniff.kind != MediaKind.IMAGE) return null
        val mime = sniff.mime ?: return null
        if (sniff.animated) return canvas(bytes, mime)
        val bitmap = decodeUpright(bytes, sniff.format) ?: return null
        val width = bitmap.width.toUInt()
        val height = bitmap.height.toUInt()
        return when (
            val plan = planCover(sniff.format, width, height, bitmap.hasAlpha(), coverWidth, coverHeight, ENCODABLE)
        ) {
            CoverPlan.Keep -> ScaledImage(bytes, mime, width, height)
            is CoverPlan.Encode ->
                encode(resize(bitmap, plan.width.toInt(), plan.height.toInt()), plan.format, plan.mime, plan.quality)
        }
    }

    private fun canvas(bytes: ByteArray, mime: String): ScaledImage? {
        val options = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)
        if (options.outWidth <= 0 || options.outHeight <= 0) return null
        return ScaledImage(bytes, mime, options.outWidth.toUInt(), options.outHeight.toUInt())
    }

    private fun decodeUpright(bytes: ByteArray, format: MediaFormat): Bitmap? = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.P -> decodeWithImageDecoder(bytes)
        format == MediaFormat.HEIF -> null
        else -> BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.let { orient(it, exifOrientation(bytes)) }
    }

    @RequiresApi(Build.VERSION_CODES.P)
    private fun decodeWithImageDecoder(bytes: ByteArray): Bitmap =
        ImageDecoder.decodeBitmap(ImageDecoder.createSource(ByteBuffer.wrap(bytes))) { decoder, _, _ ->
            decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        }

    private fun exifOrientation(bytes: ByteArray): Int = runCatching {
        ExifInterface(ByteArrayInputStream(bytes))
            .getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL)
    }.getOrDefault(ExifInterface.ORIENTATION_NORMAL)

    private fun orient(bitmap: Bitmap, orientation: Int): Bitmap {
        val transform = exifTransform(orientation)
        if (transform.degrees == 0f && !transform.mirrored) return bitmap
        val matrix = Matrix().apply {
            postRotate(transform.degrees)
            if (transform.mirrored) postScale(-1f, 1f)
        }
        return Bitmap.createBitmap(bitmap, 0, 0, bitmap.width, bitmap.height, matrix, true)
    }

    private fun resize(bitmap: Bitmap, width: Int, height: Int): Bitmap {
        val halved = generateSequence(bitmap) { current ->
            if (current.width / 2 >= width && current.height / 2 >= height) {
                Bitmap.createScaledBitmap(current, current.width / 2, current.height / 2, true)
            } else {
                null
            }
        }.last()
        if (halved.width == width && halved.height == height) return halved
        return Bitmap.createScaledBitmap(halved, width, height, true)
    }

    private companion object {
        val ENCODABLE = listOf(StillFormat.JPEG, StillFormat.PNG, StillFormat.WEBP)
    }
}

private fun encode(bitmap: Bitmap, format: StillFormat, mime: String, quality: Float): ScaledImage? {
    val compressFormat = when (format) {
        StillFormat.JPEG -> Bitmap.CompressFormat.JPEG
        StillFormat.PNG -> Bitmap.CompressFormat.PNG
        StillFormat.WEBP -> webpFormat()
        StillFormat.GIF -> return null
    }
    val out = ByteArrayOutputStream()
    val level = (quality.coerceIn(0f, 1f) * 100f).roundToInt()
    if (!bitmap.compress(compressFormat, level, out)) return null
    return ScaledImage(out.toByteArray(), mime, bitmap.width.toUInt(), bitmap.height.toUInt())
}

@Suppress("DEPRECATION")
private fun webpFormat(): Bitmap.CompressFormat =
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) Bitmap.CompressFormat.WEBP_LOSSY else Bitmap.CompressFormat.WEBP
