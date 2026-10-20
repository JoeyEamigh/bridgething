package com.bridgething

import android.app.Activity
import kotlinx.coroutines.CompletableDeferred

internal class PendingRequests<Owner : Any> {
    private class Entry<Owner>(val owner: Owner, val result: CompletableDeferred<ActivityResult>)

    private val entries = mutableMapOf<Int, Entry<Owner>>()

    suspend fun await(code: Int, owner: Owner?, launch: suspend (Owner) -> Unit): ActivityResult {
        val result = CompletableDeferred<ActivityResult>()
        val claimed = synchronized(entries) {
            !entries.containsKey(code) && (owner == null || entries.put(code, Entry(owner, result)) == null)
        }
        check(claimed) { "busy: a system dialog is already open" }
        checkNotNull(owner) { "unsupported: there is no screen to present on" }
        try {
            launch(owner)
            return result.await()
        } finally {
            synchronized(entries) { if (entries[code]?.result === result) entries.remove(code) }
        }
    }

    fun deliver(code: Int, outcome: ActivityResult): Boolean {
        val entry = synchronized(entries) { entries.remove(code) } ?: return false
        entry.result.complete(outcome)
        return true
    }

    fun destroyed(owner: Owner, finishing: Boolean) {
        if (!finishing) return
        val abandoned = synchronized(entries) {
            entries.filterValues { it.owner === owner }.keys.mapNotNull { entries.remove(it) }
        }
        abandoned.forEach { it.result.complete(ActivityResult(Activity.RESULT_CANCELED, null)) }
    }
}
