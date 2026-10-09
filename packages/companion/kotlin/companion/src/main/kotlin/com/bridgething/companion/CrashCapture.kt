package com.bridgething.companion

import uniffi.bridgething_companion.LogStoreLevel

public object CrashCapture {
    private var installed = false

    @Synchronized
    public fun install() {
        if (installed) return
        installed = true
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            runCatching {
                CompanionLogs.store?.let { store ->
                    store.record(LogStoreLevel.FATAL, "crash", "uncaught on ${thread.name}: ${error.stackTraceToString()}")
                    store.flush()
                }
            }
            if (previous != null) {
                previous.uncaughtException(thread, error)
            } else {
                thread.threadGroup?.uncaughtException(thread, error)
            }
        }
    }
}
