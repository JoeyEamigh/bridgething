package com.bridgething

import android.app.Activity
import android.app.Application
import android.content.Intent
import android.os.Bundle
import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

public enum class ActivityRequest {
    COMPANION_DEVICE,
    MEDIA_PICK,
    DIALER_ROLE;

    internal val code: Int
        get() = 0xBA01 + ordinal
}

public class ActivityResult(public val resultCode: Int, public val data: Intent?)

public object BridgethingActivityRegistry {
    private val currentRef = AtomicReference<Activity?>(null)

    public val currentActivity: Activity?
        get() = currentRef.get()

    private val pending = PendingRequests<Activity>()

    public fun installCallbacks(application: Application) {
        application.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            override fun onActivityCreated(activity: Activity, bundle: Bundle?) {}
            override fun onActivityStarted(activity: Activity) {}
            override fun onActivityResumed(activity: Activity) { currentRef.set(activity) }
            override fun onActivityPaused(activity: Activity) {
                currentRef.compareAndSet(activity, null)
            }
            override fun onActivityStopped(activity: Activity) {}
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}
            override fun onActivityDestroyed(activity: Activity) {
                currentRef.compareAndSet(activity, null)
                pending.destroyed(activity, activity.isFinishing)
            }
        })
    }

    public suspend fun awaitResult(
        request: ActivityRequest,
        launch: (activity: Activity, requestCode: Int) -> Unit,
    ): ActivityResult = pending.await(request.code, currentActivity) { activity ->
        withContext(Dispatchers.Main.immediate) { launch(activity, request.code) }
    }

    public fun deliverResult(requestCode: Int, resultCode: Int, data: Intent?): Boolean =
        pending.deliver(requestCode, ActivityResult(resultCode, data))
}
