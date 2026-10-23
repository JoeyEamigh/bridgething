package com.bridgething

import android.app.Activity
import android.app.Application
import android.bluetooth.BluetoothDevice
import android.content.ComponentName
import android.content.Context
import android.os.Bundle
import android.provider.Settings
import android.util.Log
import com.bridgething.companion.BridgethingCompanion
import com.bridgething.companion.shell.BtLinkTransport
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import uniffi.bridgething_companion.HostInfo
import uniffi.bridgething_companion.SessionEvent

public object CompanionHolder {
    private const val TAG = "BridgethingBT"
    private val mutex = Mutex()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    @Volatile
    public var companion: BridgethingCompanion? = null
        private set

    @Volatile
    public var transport: BtLinkTransport? = null
        private set

    @Volatile
    public var eventSink: ((SessionEvent) -> Unit)? = null

    @Volatile
    public var foreground: Boolean = false
        internal set

    @Volatile
    internal var onForeground: (() -> Unit)? = null

    @Volatile
    internal var onBackground: (() -> Unit)? = null

    private var lifecycleRegistered = false
    private var startedActivities = 0

    public suspend fun ensureStarted(context: Context): BridgethingCompanion = mutex.withLock {
        (context.applicationContext as? Application)?.let { ensureLifecycleObserver(it) }
        companion?.let { return it }
        val appCtx = context.applicationContext
        BondWatcher.register(appCtx)
        val c = BridgethingCompanion(
            context = appCtx,
            host = makeHostInfo(appCtx),
            capabilities = toCoreCapabilityFlags(HybridBridgethingSessionImpl.capabilityFlags(appCtx)),
            resolveNotificationAction = { id, positive -> NotificationBridgeRegistry.listener?.actionIntent(id, positive) },
            notificationListener = ComponentName(appCtx, BridgethingNotificationListener::class.java),
            spotify = HybridBridgethingSessionImpl.spotifyConfig,
            events = { event -> eventSink?.invoke(event) },
        )
        HybridBridgethingSessionImpl.inheritHostSettings(appCtx, c.session)
        c.start()
        companion = c
        transport = c.transport
        NotificationBridgeRegistry.companion = c
        NotificationBridgeRegistry.backend = c.notifications
        reconnectAssociated(appCtx)
        c
    }

    public fun reconnectAssociated(context: Context) {
        val link = transport
        if (link == null) {
            Log.w(TAG, "reconnectAssociated: no transport yet")
            return
        }
        val ba = (context.applicationContext.getSystemService(Context.BLUETOOTH_SERVICE) as? android.bluetooth.BluetoothManager)?.adapter
        if (ba == null) {
            Log.w(TAG, "reconnectAssociated: bluetooth adapter unavailable")
            return
        }
        val macs = CompanionDevicePicker.associations(context.applicationContext)
        Log.i(TAG, "reconnectAssociated: ${macs.size} association(s) $macs")
        for (mac in macs) {
            val device = runCatching { ba.getRemoteDevice(mac.uppercase()) }.getOrNull()
            if (device == null) {
                Log.w(TAG, "reconnectAssociated: getRemoteDevice($mac) failed")
                continue
            }
            val bond = runCatching { device.bondState }.getOrDefault(BluetoothDevice.BOND_NONE)
            if (bond != BluetoothDevice.BOND_BONDED) {
                Log.i(TAG, "reconnectAssociated: skipping $mac, not bonded (bondState=$bond)")
                continue
            }
            Log.i(TAG, "reconnectAssociated: connecting $mac")
            scope.launch {
                runCatching { link.connect(device) }
                    .onFailure { Log.w(TAG, "reconnectAssociated: connect($mac) failed: ${it.message}") }
            }
        }
    }

    public suspend fun connectBonded(context: Context, device: BluetoothDevice) {
        if (transport == null) runCatching { ensureStarted(context) }
        val link = transport ?: run {
            Log.w(TAG, "connectBonded: no transport for ${device.address}")
            return
        }
        Log.i(TAG, "connectBonded: ${device.address} bonded, connecting")
        runCatching { link.connect(device) }
            .onFailure { Log.w(TAG, "connectBonded: connect(${device.address}) failed: ${it.message}") }
    }

    public suspend fun forgetDevice(mac: String) {
        val link = transport ?: return
        runCatching { link.forget(mac.uppercase()) }
            .onFailure { Log.w(TAG, "forgetDevice($mac) failed: ${it.message}") }
    }

    private fun ensureLifecycleObserver(app: Application) {
        if (lifecycleRegistered) return
        lifecycleRegistered = true
        if (BridgethingActivityRegistry.currentActivity != null) {
            startedActivities = 1
            onForeground?.invoke() ?: run { foreground = true }
        }
        app.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            override fun onActivityStarted(activity: Activity) {
                startedActivities++
                resumeForeground()
            }

            override fun onActivityStopped(activity: Activity) {
                startedActivities = (startedActivities - 1).coerceAtLeast(0)
                if (startedActivities == 0) {
                    foreground = false
                    onBackground?.invoke()
                }
            }

            private fun resumeForeground() {
                val resume = onForeground
                if (resume != null) resume() else foreground = true
            }

            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}
            override fun onActivityResumed(activity: Activity) {}
            override fun onActivityPaused(activity: Activity) {}
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}
            override fun onActivityDestroyed(activity: Activity) {}
        })
    }

    @Suppress("HardwareIds")
    internal fun makeHostInfo(context: Context): HostInfo = HybridBridgethingSessionImpl.hostInfo.copy(
        osVersion = android.os.Build.VERSION.RELEASE ?: "",
        hostIdentifier = Settings.Secure.getString(context.contentResolver, Settings.Secure.ANDROID_ID) ?: "",
    )
}
