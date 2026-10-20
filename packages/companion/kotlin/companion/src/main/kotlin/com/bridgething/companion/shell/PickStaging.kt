package com.bridgething.companion.shell

import java.io.File
import java.io.InputStream
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

public class PickStaging(cacheDir: File) {
    private val dir: File = File(cacheDir, "bridgething-pick")

    public fun sweep() {
        dir.deleteRecursively()
    }

    public suspend fun <T> staged(extension: String, open: () -> InputStream, use: suspend (File) -> T): T =
        withContext(Dispatchers.IO) {
            dir.mkdirs()
            val target = File(dir, "${UUID.randomUUID()}.$extension")
            try {
                open().use { source -> target.outputStream().use { sink -> source.copyTo(sink) } }
                use(target)
            } finally {
                target.delete()
            }
        }
}
