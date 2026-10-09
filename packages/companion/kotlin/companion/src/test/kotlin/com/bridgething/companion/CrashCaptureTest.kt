package com.bridgething.companion

import java.io.File
import java.nio.file.Files
import org.junit.jupiter.api.Assertions.assertSame
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test

class CrashCaptureTest {
    @Test
    fun anUncaughtThrowIsPersistedAndStillReachesTheHandlerBeforeIt() {
        val root = Files.createTempDirectory("crash-capture").toFile()
        val store = CompanionLogs.install(root)
        val original = Thread.getDefaultUncaughtExceptionHandler()
        var reached: Throwable? = null
        Thread.setDefaultUncaughtExceptionHandler { _, error -> reached = error }
        try {
            CrashCapture.install()
            val boom = IllegalStateException("boom")
            val worker = Thread({ throw boom }, "crash-worker")
            worker.start()
            worker.join()

            assertSame(boom, reached)
            val bundle = File(root, "bundle.txt")
            store.exportTo(bundle.path, null)
            val text = bundle.readText()
            assertTrue(
                text.contains(" F crash: uncaught on crash-worker: java.lang.IllegalStateException: boom"),
                text,
            )
            assertTrue(store.archives().first { it.current }.pinned, "a crash pins the launch holding it")
        } finally {
            Thread.setDefaultUncaughtExceptionHandler(original)
        }
    }
}
