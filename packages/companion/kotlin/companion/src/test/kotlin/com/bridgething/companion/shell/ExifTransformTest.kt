package com.bridgething.companion.shell

import android.media.ExifInterface
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test

class ExifTransformTest {
    @Test
    fun everyExifOrientationMapsToARotationThenMirror() {
        val expected = mapOf(
            ExifInterface.ORIENTATION_UNDEFINED to ExifTransform(0f, false),
            ExifInterface.ORIENTATION_NORMAL to ExifTransform(0f, false),
            ExifInterface.ORIENTATION_FLIP_HORIZONTAL to ExifTransform(0f, true),
            ExifInterface.ORIENTATION_ROTATE_180 to ExifTransform(180f, false),
            ExifInterface.ORIENTATION_FLIP_VERTICAL to ExifTransform(180f, true),
            ExifInterface.ORIENTATION_TRANSPOSE to ExifTransform(90f, true),
            ExifInterface.ORIENTATION_ROTATE_90 to ExifTransform(90f, false),
            ExifInterface.ORIENTATION_TRANSVERSE to ExifTransform(270f, true),
            ExifInterface.ORIENTATION_ROTATE_270 to ExifTransform(270f, false),
        )
        expected.forEach { (orientation, transform) -> assertEquals(transform, exifTransform(orientation)) }
    }

    @Test
    fun rotatingThenMirroringLandsEachCornerWhereExifSaysItBelongs() {
        val corners = listOf(0 to 0, 1 to 0, 0 to 1, 1 to 1)
        val stored = mapOf(
            ExifInterface.ORIENTATION_FLIP_HORIZONTAL to listOf(1 to 0, 0 to 0, 1 to 1, 0 to 1),
            ExifInterface.ORIENTATION_ROTATE_180 to listOf(1 to 1, 0 to 1, 1 to 0, 0 to 0),
            ExifInterface.ORIENTATION_FLIP_VERTICAL to listOf(0 to 1, 1 to 1, 0 to 0, 1 to 0),
            ExifInterface.ORIENTATION_TRANSPOSE to listOf(0 to 0, 0 to 1, 1 to 0, 1 to 1),
            ExifInterface.ORIENTATION_ROTATE_90 to listOf(1 to 0, 1 to 1, 0 to 0, 0 to 1),
            ExifInterface.ORIENTATION_TRANSVERSE to listOf(1 to 1, 1 to 0, 0 to 1, 0 to 0),
            ExifInterface.ORIENTATION_ROTATE_270 to listOf(0 to 1, 0 to 0, 1 to 1, 1 to 0),
        )
        stored.forEach { (orientation, upright) ->
            val transform = exifTransform(orientation)
            assertEquals(upright, corners.map { apply(transform, it) }, "orientation $orientation")
        }
    }

    private fun apply(transform: ExifTransform, corner: Pair<Int, Int>): Pair<Int, Int> {
        val rotated = generateSequence(corner) { (x, y) -> (1 - y) to x }
            .elementAt((transform.degrees / 90f).toInt())
        return if (transform.mirrored) (1 - rotated.first) to rotated.second else rotated
    }
}
