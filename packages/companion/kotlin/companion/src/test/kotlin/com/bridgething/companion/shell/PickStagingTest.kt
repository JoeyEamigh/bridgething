package com.bridgething.companion.shell

import java.io.ByteArrayInputStream
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.nio.file.Files
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.jupiter.api.Assertions.assertArrayEquals
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertThrows
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

private class BreaksPartway(private val good: Int) : InputStream() {
    private var served = 0

    override fun read(): Int {
        if (served == good) throw IOException("the network dropped")
        served++
        return 7
    }
}

class PickStagingTest {
    private val cacheDir: File = Files.createTempDirectory("pick-staging").toFile().apply { deleteOnExit() }
    private val staging = PickStaging(cacheDir)

    private fun leftovers(): List<File> = cacheDir.walkTopDown().filter { it.isFile }.toList()

    @Test
    fun aStagedPickHoldsEveryByteUnderItsExtensionAndIsGoneOnceItsUseReturns() = runBlocking {
        val bytes = ByteArray(200_000) { it.toByte() }
        val staged = staging.staged("mp4", { ByteArrayInputStream(bytes) }) { file ->
            assertEquals("mp4", file.extension)
            assertArrayEquals(bytes, file.readBytes())
            file
        }
        assertFalse(staged.exists())
        assertEquals(emptyList<File>(), leftovers())
    }

    @Test
    fun twoPicksNeverShareAFile() = runBlocking {
        val first = staging.staged("jpg", { ByteArrayInputStream(byteArrayOf(1)) }) { it }
        val second = staging.staged("jpg", { ByteArrayInputStream(byteArrayOf(2)) }) { it }
        assertFalse(first == second)
    }

    @Test
    fun aCopyThatBreaksPartwayLeavesNothingBehindAndNeverReachesItsUse() {
        var used = false
        val failure = assertThrows(IOException::class.java) {
            runBlocking { staging.staged("mp4", { BreaksPartway(100_000) }) { used = true } }
        }
        assertEquals("the network dropped", failure.message)
        assertFalse(used)
        assertEquals(emptyList<File>(), leftovers())
    }

    @Test
    fun aSourceThatCannotBeOpenedLeavesNothingBehind() {
        assertThrows(IOException::class.java) {
            runBlocking { staging.staged("bin", { throw IOException("gone") }) { } }
        }
        assertEquals(emptyList<File>(), leftovers())
    }

    @Test
    fun aUseThatFailsStillRemovesTheStagedFile() {
        var seen: File? = null
        assertThrows(IllegalStateException::class.java) {
            runBlocking {
                staging.staged("png", { ByteArrayInputStream(byteArrayOf(1, 2, 3)) }) { file ->
                    seen = file
                    assertTrue(file.exists())
                    error("too_large: over the limit")
                }
            }
        }
        assertFalse(checkNotNull(seen).exists())
        assertEquals(emptyList<File>(), leftovers())
    }

    @Test
    fun aUseThatIsCancelledStillRemovesTheStagedFile() = runBlocking {
        val seen = CompletableDeferred<File>()
        val job = launch {
            staging.staged("mp4", { ByteArrayInputStream(byteArrayOf(1, 2, 3)) }) { file ->
                seen.complete(file)
                awaitCancellation()
            }
        }
        val file = seen.await()
        assertTrue(file.exists())
        job.cancelAndJoin()
        assertFalse(file.exists())
        assertEquals(emptyList<File>(), leftovers())
    }

    @Test
    fun aSweepRemovesWhatAKilledProcessLeftBehind() = runBlocking {
        val dir = staging.staged("mp4", { ByteArrayInputStream(byteArrayOf(1)) }) { checkNotNull(it.parentFile) }
        val orphan = File(dir, "orphan.mp4").apply { writeBytes(ByteArray(4096)) }
        val unrelated = File(cacheDir, "unrelated.bin").apply { writeBytes(byteArrayOf(9)) }

        PickStaging(cacheDir).sweep()

        assertFalse(orphan.exists())
        assertTrue(unrelated.exists())
        val again = staging.staged("mp4", { ByteArrayInputStream(byteArrayOf(5)) }) { it.readBytes() }
        assertArrayEquals(byteArrayOf(5), again)
    }

    @Test
    fun aSweepOfACacheThatNeverStagedAnythingIsQuiet() {
        PickStaging(File(cacheDir, "fresh")).sweep()
        assertEquals(emptyList<File>(), leftovers())
    }
}
