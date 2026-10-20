package com.bridgething.companion.shell

import android.media.MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR
import android.media.MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CQ
import android.media.MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_VBR
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Test

class BitrateModeTest {
    private fun encoder(vararg modes: Int): (Int) -> Boolean = { mode -> mode in modes }

    @Test
    fun aFittedPlanAsksForConstantBitrateWhenSomeEncoderCanHoldIt() {
        val encoders = listOf(encoder(BITRATE_MODE_VBR), encoder(BITRATE_MODE_VBR, BITRATE_MODE_CBR))
        assertEquals(BITRATE_MODE_CBR, bitrateMode(fitted = true, encoders = encoders))
    }

    @Test
    fun aFittedPlanStaysVariableWhenNoEncoderCanHoldAConstantBitrate() {
        val encoders = listOf(encoder(BITRATE_MODE_VBR), encoder(BITRATE_MODE_VBR, BITRATE_MODE_CQ))
        assertEquals(BITRATE_MODE_VBR, bitrateMode(fitted = true, encoders = encoders))
        assertEquals(BITRATE_MODE_VBR, bitrateMode(fitted = true, encoders = emptyList()))
    }

    @Test
    fun aPlanQualitySetIsAlwaysVariable() {
        assertEquals(BITRATE_MODE_VBR, bitrateMode(fitted = false, encoders = listOf(encoder(BITRATE_MODE_CBR))))
    }
}
