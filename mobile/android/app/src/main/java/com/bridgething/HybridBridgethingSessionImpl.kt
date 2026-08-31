package com.bridgething

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.core.content.FileProvider
import com.bridgething.companion.BridgethingCompanion
import com.bridgething.companion.CompanionLogs
import com.bridgething.companion.LogcatCapture
import com.bridgething.session.BridgethingSessionBackend
import com.margelo.nitro.bridgething.session.BridgethingActiveWebapp
import com.margelo.nitro.bridgething.session.BridgethingAncsAuthStatus
import com.margelo.nitro.bridgething.session.BridgethingAncsSetupKind
import com.margelo.nitro.bridgething.session.BridgethingAncsSetupResult
import com.margelo.nitro.bridgething.session.BridgethingBtBondState
import com.margelo.nitro.bridgething.session.BridgethingBtDevice
import com.margelo.nitro.bridgething.session.BridgethingCapabilityFlags
import com.margelo.nitro.bridgething.session.BridgethingCompanionDebug
import com.margelo.nitro.bridgething.session.BridgethingConfigEntry
import com.margelo.nitro.bridgething.session.BridgethingDeviceLogLine
import com.margelo.nitro.bridgething.session.BridgethingDeviceMeta
import com.margelo.nitro.bridgething.session.BridgethingDeviceWebappsEntry
import com.margelo.nitro.bridgething.session.BridgethingDocEntry
import com.margelo.nitro.bridgething.session.BridgethingLogArchive
import com.margelo.nitro.bridgething.session.BridgethingNowPlaying
import com.margelo.nitro.bridgething.session.BridgethingOtaAvailable
import com.margelo.nitro.bridgething.session.BridgethingOtaManifest
import com.margelo.nitro.bridgething.session.BridgethingOtaPollConfig
import com.margelo.nitro.bridgething.session.BridgethingOtaPollStatus
import com.margelo.nitro.bridgething.session.BridgethingOtaProgress
import com.margelo.nitro.bridgething.session.BridgethingOtaRun
import com.margelo.nitro.bridgething.session.BridgethingProviderInfo
import com.margelo.nitro.bridgething.session.BridgethingResumeTarget
import com.margelo.nitro.bridgething.session.BridgethingSessionPeer
import com.margelo.nitro.bridgething.session.BridgethingSessionSnapshot
import com.margelo.nitro.bridgething.session.BridgethingVoiceModelState
import com.margelo.nitro.bridgething.session.BridgethingVoiceTurn
import com.margelo.nitro.bridgething.session.BridgethingWebappIcon
import com.margelo.nitro.bridgething.session.BridgethingWebappInfo
import com.margelo.nitro.bridgething.session.BridgethingWebappSlot
import com.margelo.nitro.bridgething.session.BridgethingWebappSlots
import java.io.File
import java.net.URI
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import uniffi.bridgething_companion.ArtifactDigest
import uniffi.bridgething_companion.CompanionException
import uniffi.bridgething_companion.CompanionSession
import uniffi.bridgething_companion.HostInfo
import uniffi.bridgething_companion.LinkDevice
import uniffi.bridgething_companion.LogOrigin
import uniffi.bridgething_companion.SessionEvent
import uniffi.bridgething_companion.SpotifyProviderConfig
import uniffi.bridgething_companion.WebappResourceKind

public class HybridBridgethingSessionImpl(
    private val context: Context,
) : BridgethingSessionBackend {

    public companion object {
        public var hostInfo: HostInfo = HostInfo(
            appName = "bridgething",
            appVersion = "0.0.0",
            osName = "Android",
            osVersion = "",
            hostIdentifier = "",
        )

        public var spotifyConfig: SpotifyProviderConfig? = null

        private const val PREFS_NAME = "bridgething.session"
        private const val VOICE_MODEL_KEY = "caps.voiceModel"
        private const val REQUEST_DIALER_ROLE = 0xBA02
        private const val AUTO_RESUME_PREFIX = "autoresume."
        private const val RESUME_TARGET_PREFIX = "resumetarget."
        private const val PROVIDER_PRIORITY_KEY = "providerPriority"
        private const val DEV_LANE_DEVICE_ID = "dev-gateway"

        internal fun capabilityFlags(context: Context): BridgethingCapabilityFlags {
            val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            if (!prefs.getBoolean("caps.configured", false)) {
                return BridgethingCapabilityFlags(
                    geo = true,
                    notifications = true,
                    netFetch = true,
                    netWs = true,
                    audioTts = true,
                    voiceModel = true,
                )
            }
            return BridgethingCapabilityFlags(
                geo = prefs.getBoolean("caps.geo", true),
                notifications = prefs.getBoolean("caps.notifications", true),
                netFetch = prefs.getBoolean("caps.netFetch", true),
                netWs = prefs.getBoolean("caps.netWs", true),
                audioTts = prefs.getBoolean("caps.audioTts", true),
                voiceModel = prefs.getBoolean(VOICE_MODEL_KEY, true),
            )
        }
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val stateLock = Mutex()
    private var companion: BridgethingCompanion? = null

    private val foregroundGen = AtomicLong(0)

    @Volatile
    private var logStreamingDesired: Boolean = false

    @Volatile
    private var localLogStreamingDesired: Boolean = false

    @Volatile
    private var onProvidersChanged: ((Array<BridgethingProviderInfo>) -> Unit)? = null

    @Volatile
    private var onPeerConnected: ((BridgethingSessionPeer) -> Unit)? = null

    @Volatile
    private var onPeerDisconnected: ((String) -> Unit)? = null

    @Volatile
    private var onPeerLinkFailed: ((BridgethingSessionPeer) -> Unit)? = null

    @Volatile
    private var onNowPlayingChanged: ((BridgethingNowPlaying?) -> Unit)? = null

    @Volatile
    private var onAncsAuthStatusChanged: ((String, BridgethingAncsAuthStatus) -> Unit)? = null

    @Volatile
    private var onLog: ((String, String, String) -> Unit)? = null

    @Volatile
    private var onWebappsChanged: ((BridgethingDeviceWebappsEntry) -> Unit)? = null

    @Volatile
    private var onWebappDocChanged: ((String, String, String, String?) -> Unit)? = null

    @Volatile
    private var onDeviceMetaChanged: ((String, BridgethingDeviceMeta) -> Unit)? = null

    @Volatile
    private var onVoiceModelStateChanged: ((BridgethingVoiceModelState) -> Unit)? = null

    @Volatile
    private var onCompanionUpdateProgress: ((Double, Double) -> Unit)? = null

    @Volatile
    private var onVoiceTurnChanged: ((BridgethingVoiceTurn) -> Unit)? = null

    @Volatile
    private var onOtaRunChanged: ((BridgethingOtaRun) -> Unit)? = null

    @Volatile
    private var onOtaAvailableChanged: ((BridgethingOtaAvailable) -> Unit)? = null

    @Volatile
    private var onOtaPollChanged: ((BridgethingOtaPollStatus) -> Unit)? = null

    @Volatile
    private var onResumed: ((BridgethingSessionSnapshot) -> Unit)? = null

    private val prefs by lazy {
        context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
    }

    init {
        CompanionHolder.onForeground = { resumeForeground() }
        CompanionHolder.onBackground = { foregroundGen.incrementAndGet() }
    }

    override suspend fun start() {
        CompanionHolder.eventSink = { event -> safeEmit { handleSessionEvent(event) } }
        val c = CompanionHolder.ensureStarted(context)
        val firstAttach = stateLock.withLock {
            if (companion != null) return@withLock false
            companion = c
            true
        }
        if (!firstAttach) return

        if (localLogStreamingDesired) LogcatCapture.setInbox(c.session.logInbox())
        if (logStreamingDesired) c.session.setDeviceLogStreaming(true)
        replayHostSettings(c.session)

        CompanionDevicePicker.startObservingPresence(context)
        if (CompanionDevicePicker.associations(context.applicationContext).isNotEmpty()) {
            BridgethingConnectionService.start(context)
        }
        connectDevGateway(c.session)
    }

    override suspend fun stop() {
        val prior = stateLock.withLock {
            val held = companion
            companion = null
            held
        }
        CompanionHolder.eventSink = null
        LogcatCapture.setInbox(null)
        emitNowPlaying(null)
    }

    private fun handleSessionEvent(event: SessionEvent) {
        val foreground = CompanionHolder.foreground
        when (event) {
            is SessionEvent.ProvidersChanged ->
                if (foreground) onProvidersChanged?.invoke(event.providers.map(::toRnProviderInfo).toTypedArray())
            is SessionEvent.PeerConnected ->
                if (foreground) onPeerConnected?.invoke(toRnPeer(event.peer))
            is SessionEvent.PeerDisconnected ->
                if (foreground) onPeerDisconnected?.invoke(event.deviceId)
            is SessionEvent.PeerLinkFailed ->
                if (foreground) onPeerLinkFailed?.invoke(toRnPeer(event.peer))
            is SessionEvent.NowPlayingChanged ->
                emitNowPlaying(event.nowPlaying?.let(::toRnNowPlaying))
            is SessionEvent.AncsAuthStatusChanged ->
                if (foreground) onAncsAuthStatusChanged?.invoke(event.deviceId, toRnAncsAuthStatus(event.status))
            is SessionEvent.Log -> {
                val line = "[${event.target}] ${event.message}"
                val level = toLevelName(event.level)
                if (event.origin == LogOrigin.DEVICE) {
                    CompanionLogs.store?.record(toStoreLevel(event.level), DAEMON_LABEL, line)
                }
                if (foreground) onLog?.invoke(toOriginName(event.origin), level, line)
            }
            is SessionEvent.WebappsChanged ->
                if (foreground) onWebappsChanged?.invoke(toRnWebappsEntry(event.entry))
            is SessionEvent.WebappDocChanged ->
                if (foreground) {
                    onWebappDocChanged?.invoke(event.deviceId, event.webappId.lowercase(), event.key, event.value)
                }
            is SessionEvent.DeviceMetaChanged ->
                if (foreground) onDeviceMetaChanged?.invoke(event.deviceId, toRnDeviceMeta(event.meta))
            is SessionEvent.VoiceModelStateChanged ->
                if (foreground) onVoiceModelStateChanged?.invoke(toRnVoiceModelState(event.state))
            is SessionEvent.VoiceTurnChanged ->
                if (foreground) onVoiceTurnChanged?.invoke(toRnVoiceTurn(event.turn))
            is SessionEvent.OtaRunChanged ->
                if (foreground) onOtaRunChanged?.invoke(toRnOtaRun(event.run))
            is SessionEvent.OtaAvailableChanged ->
                if (foreground) onOtaAvailableChanged?.invoke(toRnOtaAvailable(event.available))
            is SessionEvent.OtaPollChanged ->
                if (foreground) onOtaPollChanged?.invoke(toRnOtaPollStatus(event.status))
            is SessionEvent.CompanionUpdateProgress ->
                if (foreground) {
                    onCompanionUpdateProgress?.invoke(event.received.toDouble(), event.total.toDouble())
                }
            is SessionEvent.Resumed -> {
                val gen = foregroundGen.get()
                scope.launch {
                    val snap = runCatching { snapshot() }.getOrNull() ?: return@launch
                    if (foregroundGen.get() != gen) return@launch
                    safeEmit { if (CompanionHolder.foreground) onResumed?.invoke(snap) }
                }
            }
        }
    }

    override suspend fun snapshot(): BridgethingSessionSnapshot = toRnSnapshot(requireSession().snapshot())

    override suspend fun availableProviders(): Array<BridgethingProviderInfo> =
        requireSession().availableProviders().map(::toRnProviderInfo).toTypedArray()

    override suspend fun connectProvider(id: String) {
        requireSession().connectProvider(id)
    }

    override suspend fun cancelAuth(id: String) {
        requireSession().cancelAuth(id)
    }

    override suspend fun disconnectProvider(id: String) {
        requireSession().disconnectProvider(id)
    }

    override suspend fun setProviderPriority(ids: Array<String>) {
        prefs.edit().putString(PROVIDER_PRIORITY_KEY, ids.joinToString(",")).apply()
        requireSession().setProviderPriority(ids.toList())
    }

    override suspend fun deviceLogSnapshot(limit: Double): Array<BridgethingDeviceLogLine> =
        requireSession().deviceLogSnapshot(limit.toInt().coerceAtLeast(0).toUInt())
            .map {
                BridgethingDeviceLogLine(
                    seq = it.seq.toDouble(),
                    ts = it.tsUnixMs.toDouble(),
                    origin = toOriginName(it.origin),
                    level = toLevelName(it.level),
                    message = "[${it.target}] ${it.message}",
                )
            }
            .toTypedArray()

    override suspend fun persistedLogSize(): Double = withContext(Dispatchers.IO) {
        CompanionLogs.store?.retainedBytes()?.toDouble() ?: 0.0
    }

    override suspend fun logArchives(): Array<BridgethingLogArchive> = withContext(Dispatchers.IO) {
        (CompanionLogs.store?.archives() ?: emptyList())
            .map {
                BridgethingLogArchive(
                    id = it.id,
                    startedAt = it.startedAtMs.toDouble(),
                    bytes = it.bytes.toDouble(),
                    pinned = it.pinned,
                    current = it.current,
                )
            }
            .toTypedArray()
    }

    override suspend fun logArchiveLines(archiveId: String, limit: Double): Array<BridgethingDeviceLogLine> =
        withContext(Dispatchers.IO) {
            (CompanionLogs.store?.read(archiveId, limit.toInt().coerceAtLeast(0).toUInt()) ?: emptyList())
                .mapIndexed { index, line ->
                    BridgethingDeviceLogLine(
                        seq = index.toDouble(),
                        ts = line.tsUnixMs.toDouble(),
                        origin = if (line.label == DAEMON_LABEL) "device" else "local",
                        level = toLevelName(line.level),
                        message = if (line.label.isEmpty()) line.message else "[${line.label}] ${line.message}",
                    )
                }
                .toTypedArray()
        }

    override suspend fun deleteLogArchive(archiveId: String): Unit = withContext(Dispatchers.IO) {
        CompanionLogs.store?.delete(archiveId)
        Unit
    }

    override suspend fun clearPersistedLogs(): Unit = withContext(Dispatchers.IO) { CompanionLogs.store?.clear() ?: Unit }

    override suspend fun companionDebug(): BridgethingCompanionDebug =
        toRnCompanionDebug(requireSession().companionDebug())

    override suspend fun enableAncsNotifications(deviceId: String): BridgethingAncsSetupResult =
        BridgethingAncsSetupResult(
            kind = BridgethingAncsSetupKind.UNSUPPORTED,
            authStatus = BridgethingAncsAuthStatus.UNKNOWN,
            message = null,
        )

    override suspend fun ancsAuthStatus(deviceId: String): BridgethingAncsAuthStatus =
        requireSession().snapshot().ancsAuthStatuses
            .firstOrNull { it.deviceId == deviceId }
            ?.let { toRnAncsAuthStatus(it.status) }
            ?: BridgethingAncsAuthStatus.UNKNOWN

    override suspend fun listWebapps(deviceId: String): Array<BridgethingWebappInfo> =
        requireSession().listWebapps(deviceId).visible().map(::toRnWebappInfo).toTypedArray()

    override suspend fun currentWebapp(deviceId: String): BridgethingActiveWebapp? =
        requireSession().currentWebapp(deviceId)?.let(::toRnActiveWebapp)

    override suspend fun installWebapp(deviceId: String, sourceUri: String): BridgethingWebappInfo {
        val session = requireSession()
        val info = when (URI(sourceUri).scheme?.lowercase()) {
            "file" -> session.installWebapp(deviceId, File(URI(sourceUri)).absolutePath, null)
            "http", "https" -> session.installWebappFromUrl(deviceId, sourceUri, null, sourceUri)
            else -> throw IllegalArgumentException("invalid archive uri")
        }
        return toRnWebappInfo(info)
    }

    override suspend fun installWebappFromUrl(
        deviceId: String,
        url: String,
        sha256: String,
        size: Double,
        provenance: String?,
        webappId: String?,
        webappName: String?,
    ): BridgethingWebappInfo = toRnWebappInfo(
        requireSession().installWebappFromUrl(
            deviceId,
            url,
            ArtifactDigest(size = size.toLong().toULong(), sha256 = sha256.lowercase()),
            provenance,
        ),
    )

    override suspend fun uninstallWebapp(deviceId: String, id: String) {
        requireSession().uninstallWebapp(deviceId, id)
    }

    override suspend fun switchWebapp(deviceId: String, id: String) {
        requireSession().switchWebapp(deviceId, id)
    }

    override suspend fun getWebappSlots(deviceId: String): BridgethingWebappSlots =
        toRnWebappSlots(requireSession().webappSlots(deviceId))

    override suspend fun setWebappSlot(
        deviceId: String,
        slot: BridgethingWebappSlot,
        id: String?,
    ): BridgethingWebappSlots = toRnWebappSlots(requireSession().setWebappSlot(deviceId, toCoreWebappSlot(slot), id))

    override suspend fun webappIcon(deviceId: String, id: String): BridgethingWebappIcon? {
        val resolved = try {
            requireSession().webappResource(deviceId, id, WebappResourceKind.ICON, null)
        } catch (e: CompanionException.ResourceNotAvailable) {
            return null
        }
        val file = File(resolved.path)
        return if (resolved.mime == "image/svg+xml") {
            BridgethingWebappIcon(fileUri = null, svg = file.readText(), mime = resolved.mime)
        } else {
            BridgethingWebappIcon(fileUri = Uri.fromFile(file).toString(), svg = null, mime = resolved.mime)
        }
    }

    override suspend fun webappSettingsMarkup(
        deviceId: String,
        id: String,
        origin: BridgethingResourceOrigin?,
    ): String {
        val resolved =
            requireSession().webappResource(
                deviceId,
                id,
                WebappResourceKind.SETTINGS,
                origin?.let { WebappResourceOrigin(it.url, it.sha256, it.size.toULong(), it.mime) },
            )
        return File(resolved.path).readText()
    }

    override suspend fun listWebappConfig(deviceId: String, id: String): Array<BridgethingConfigEntry> =
        requireSession().listWebappConfig(deviceId, id)
            .map { BridgethingConfigEntry(it.key, it.value) }
            .toTypedArray()

    override suspend fun setWebappConfigField(deviceId: String, id: String, key: String, value: String) {
        requireSession().setWebappConfigField(deviceId, id, key, value)
    }

    override suspend fun deleteWebappConfigField(deviceId: String, id: String, key: String) {
        requireSession().deleteWebappConfigField(deviceId, id, key)
    }

    override suspend fun getWebappDoc(deviceId: String, id: String, key: String): String? =
        requireSession().getWebappDoc(deviceId, id, key)

    override suspend fun listWebappDoc(deviceId: String, id: String): Array<BridgethingDocEntry> =
        requireSession().listWebappDoc(deviceId, id)
            .map { BridgethingDocEntry(it.key, it.value) }
            .toTypedArray()

    override suspend fun setWebappDoc(deviceId: String, id: String, key: String, value: String) {
        requireSession().setWebappDoc(deviceId, id, key, value)
    }

    override suspend fun deleteWebappDoc(deviceId: String, id: String, key: String) {
        requireSession().deleteWebappDoc(deviceId, id, key)
    }

    override suspend fun setCapabilityFlags(flags: BridgethingCapabilityFlags) {
        saveCapabilityFlags(flags)
        requireSession().setCapabilityFlags(toCoreCapabilityFlags(flags))
    }

    override suspend fun voiceModelState(): BridgethingVoiceModelState =
        toRnVoiceModelState(requireSession().snapshot().voiceModel)

    override suspend fun downloadVoiceModel() {
        requireSession().downloadVoiceModel()
    }

    override suspend fun setDeviceAutoResume(deviceId: String, enabled: Boolean) {
        prefs.edit().putBoolean("$AUTO_RESUME_PREFIX$deviceId", enabled).apply()
        requireSession().setDeviceAutoResume(deviceId, enabled)
    }

    override suspend fun isDeviceAutoResumeEnabled(deviceId: String): Boolean =
        prefs.getBoolean("$AUTO_RESUME_PREFIX$deviceId", true)

    override suspend fun setDeviceResumeTarget(deviceId: String, target: BridgethingResumeTarget) {
        prefs.edit().putString("$RESUME_TARGET_PREFIX$deviceId", target.name).apply()
        requireSession().setDeviceResumeTarget(deviceId, toCoreResumeTarget(target))
    }

    override suspend fun deviceResumeTarget(deviceId: String): BridgethingResumeTarget =
        prefs.getString("$RESUME_TARGET_PREFIX$deviceId", null)
            ?.let { raw -> BridgethingResumeTarget.entries.firstOrNull { it.name == raw } }
            ?: BridgethingResumeTarget.PHONEONLY

    override suspend fun setOtaPollConfig(config: BridgethingOtaPollConfig?) {
        saveOtaPollConfig(config)
        requireSession().setOtaPollConfig(config?.let(::toCoreOtaPollConfig))
    }

    override suspend fun checkForOtaUpdate(rootUrl: String) {
        requireSession().checkForOtaUpdate(rootUrl)
    }

    override suspend fun fetchOtaManifest(rootUrl: String): BridgethingOtaManifest =
        toRnOtaManifest(requireSession().fetchOtaManifest(rootUrl))

    override suspend fun applyOtaUpdate(deviceId: String, channel: String, version: String, rootUrl: String) {
        requireSession().applyOtaUpdate(deviceId, channel, version, rootUrl)
    }

    override fun otaRunProgress(deviceId: String, nowMs: Double): BridgethingOtaProgress? {
        val c = companion ?: return null
        return c.session.otaRunProgress(deviceId, nowMs.toLong().coerceAtLeast(0L).toULong())?.let(::toRnOtaProgress)
    }

    override suspend fun dismissOtaRun(deviceId: String) {
        requireSession().dismissOtaRun(deviceId)
    }

    override suspend fun reconnectPeer(deviceId: String) {
        stateLock.withLock { companion }?.transport?.reconnect(deviceId)
    }

    override suspend fun deviceSetNickname(deviceId: String, nickname: String) {
        requireSession().deviceSetNickname(deviceId, nickname)
    }

    override suspend fun exportLogs(archiveId: String?): String = withContext(Dispatchers.IO) {
        LogExport.writeBundle(context, archiveId).absolutePath
    }

    override suspend fun shareLogs(archiveId: String?): Boolean {
        val file = withContext(Dispatchers.IO) {
            runCatching { LogExport.writeBundle(context, archiveId) }.getOrNull()
        } ?: return false
        return withContext(Dispatchers.Main) { LogExport.share(context, file) }
    }

    override suspend fun isNotificationAccessGranted(): Boolean {
        val ctx = context.applicationContext
        return androidx.core.app.NotificationManagerCompat.getEnabledListenerPackages(ctx).contains(ctx.packageName)
    }

    override suspend fun requestNotificationAccess() {
        context.applicationContext.startActivity(
            Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS).apply { addFlags(Intent.FLAG_ACTIVITY_NEW_TASK) },
        )
    }

    override suspend fun isDefaultDialer(): Boolean {
        val ctx = context.applicationContext
        return if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
            ctx.getSystemService(android.app.role.RoleManager::class.java)
                ?.isRoleHeld(android.app.role.RoleManager.ROLE_DIALER) == true
        } else {
            val telecom = ctx.getSystemService(Context.TELECOM_SERVICE) as? android.telecom.TelecomManager
            telecom?.defaultDialerPackage == ctx.packageName
        }
    }

    override suspend fun installCompanionUpdate(url: String, filename: String, size: Double, sha256: String) {
        val path = requireSession().downloadCompanionUpdate(
            url,
            filename,
            ArtifactDigest(size = size.toLong().toULong(), sha256 = sha256.lowercase()),
        )
        val ctx = context.applicationContext
        val uri = FileProvider.getUriForFile(ctx, ctx.packageName + ".fileprovider", File(path))
        val intent = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_GRANT_READ_URI_PERMISSION)
        (BridgethingActivityRegistry.currentActivity ?: ctx).startActivity(intent)
    }

    override suspend fun requestDefaultDialer() {
        val ctx = context.applicationContext
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
            val roleManager = ctx.getSystemService(android.app.role.RoleManager::class.java) ?: return
            if (!roleManager.isRoleAvailable(android.app.role.RoleManager.ROLE_DIALER)) return
            val activity = BridgethingActivityRegistry.currentActivity ?: return
            val intent = roleManager.createRequestRoleIntent(android.app.role.RoleManager.ROLE_DIALER)
            val done = CompletableDeferred<Unit>()
            BridgethingActivityRegistry.expectResult(REQUEST_DIALER_ROLE) { _, _ -> done.complete(Unit) }
            activity.startActivityForResult(intent, REQUEST_DIALER_ROLE)
            done.await()
        } else {
            @Suppress("DEPRECATION")
            val intent = Intent(android.telecom.TelecomManager.ACTION_CHANGE_DEFAULT_DIALER)
                .putExtra(android.telecom.TelecomManager.EXTRA_CHANGE_DEFAULT_DIALER_PACKAGE_NAME, ctx.packageName)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            ctx.startActivity(intent)
        }
    }

    override suspend fun forgetCompanionDevice(mac: String) {
        val ctx = context.applicationContext
        CompanionDevicePicker.forget(ctx, mac)
        runCatching { CompanionHolder.transport?.disconnect(mac.uppercase()) }
        if (CompanionDevicePicker.associations(ctx).isEmpty()) {
            BridgethingConnectionService.stop(ctx)
        }
    }

    override suspend fun isIgnoringBatteryOptimizations(): Boolean {
        val ctx = context.applicationContext
        val pm = ctx.getSystemService(Context.POWER_SERVICE) as? android.os.PowerManager ?: return false
        return pm.isIgnoringBatteryOptimizations(ctx.packageName)
    }

    override suspend fun requestIgnoreBatteryOptimizations() {
        val ctx = context.applicationContext
        @Suppress("BatteryLife")
        val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
            .setData(Uri.parse("package:${ctx.packageName}"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        ctx.startActivity(intent)
    }

    override suspend fun revokeRuntimePermissions(permissions: Array<String>): Boolean {
        if (android.os.Build.VERSION.SDK_INT < android.os.Build.VERSION_CODES.TIRAMISU) return false
        if (permissions.isEmpty()) return false
        return try {
            context.applicationContext.revokeSelfPermissionsOnKill(permissions.toList())
            true
        } catch (e: Throwable) {
            android.util.Log.w("bridgething.session", "revokeSelfPermissionsOnKill threw: ${e.message}")
            false
        }
    }

    override suspend fun killApp() {
        android.os.Process.killProcess(android.os.Process.myPid())
    }

    override suspend fun presentPairPicker(): BridgethingBtDevice? {
        val picked = CompanionDevicePicker.pick(context.applicationContext) ?: return null
        CompanionDevicePicker.startObservingPresence(context)
        BridgethingConnectionService.start(context)

        val bonded = CompanionDevicePicker.awaitBond(context.applicationContext, picked.address)
        return BridgethingBtDevice(
            address = picked.address,
            name = picked.name,
            bondState = if (bonded) BridgethingBtBondState.BONDED else BridgethingBtBondState.NONE,
            isCarThing = picked.isCarThing,
        )
    }

    override fun setOnProvidersChanged(callback: (Array<BridgethingProviderInfo>) -> Unit) {
        onProvidersChanged = callback
    }

    override fun setOnPeerConnected(callback: (BridgethingSessionPeer) -> Unit) { onPeerConnected = callback }

    override fun setOnPeerDisconnected(callback: (String) -> Unit) { onPeerDisconnected = callback }

    override fun setOnPeerLinkFailed(callback: (BridgethingSessionPeer) -> Unit) { onPeerLinkFailed = callback }

    override fun setOnNowPlayingChanged(callback: (BridgethingNowPlaying?) -> Unit) { onNowPlayingChanged = callback }

    override fun setOnAncsAuthStatusChanged(callback: (String, BridgethingAncsAuthStatus) -> Unit) {
        onAncsAuthStatusChanged = callback
    }

    override fun setOnLog(callback: (String, String, String) -> Unit) { onLog = callback }

    override fun setLogStreamingEnabled(enabled: Boolean) {
        logStreamingDesired = enabled
        val c = companion ?: return
        scope.launch { c.session.setDeviceLogStreaming(enabled) }
    }

    override fun setLocalLogStreamingEnabled(enabled: Boolean) {
        localLogStreamingDesired = enabled
        val c = companion ?: return
        LogcatCapture.setInbox(if (enabled) c.session.logInbox() else null)
    }

    override fun setOnWebappsChanged(callback: (BridgethingDeviceWebappsEntry) -> Unit) { onWebappsChanged = callback }

    override fun setOnWebappDocChanged(callback: (String, String, String, String?) -> Unit) {
        onWebappDocChanged = callback
    }

    override fun setOnDeviceMetaChanged(callback: (String, BridgethingDeviceMeta) -> Unit) {
        onDeviceMetaChanged = callback
    }

    override fun setOnVoiceModelStateChanged(callback: (BridgethingVoiceModelState) -> Unit) {
        onVoiceModelStateChanged = callback
    }

    override fun setOnVoiceTurnChanged(callback: (BridgethingVoiceTurn) -> Unit) {
        onVoiceTurnChanged = callback
    }

    override fun setOnOtaRunChanged(callback: (BridgethingOtaRun) -> Unit) { onOtaRunChanged = callback }

    override fun setOnOtaAvailableChanged(callback: (BridgethingOtaAvailable) -> Unit) {
        onOtaAvailableChanged = callback
    }

    override fun setOnOtaPollChanged(callback: (BridgethingOtaPollStatus) -> Unit) { onOtaPollChanged = callback }

    override fun setOnCompanionUpdateProgress(callback: (Double, Double) -> Unit) { onCompanionUpdateProgress = callback }

    override fun setOnResumed(callback: (BridgethingSessionSnapshot) -> Unit) { onResumed = callback }

    public fun resumeForeground() {
        foregroundGen.incrementAndGet()
        CompanionHolder.foreground = true
        scope.launch { stateLock.withLock { companion }?.session?.resumed() }
    }

    private suspend fun requireSession(): CompanionSession =
        (stateLock.withLock { companion } ?: throw IllegalStateException("session not started")).session

    private fun emitNowPlaying(np: BridgethingNowPlaying?) {
        if (CompanionHolder.foreground) onNowPlayingChanged?.invoke(np)
    }

    private suspend fun replayHostSettings(session: CompanionSession) {
        runCatching { session.setCapabilityFlags(toCoreCapabilityFlags(loadCapabilityFlags())) }
        runCatching { session.setOtaPollConfig(loadOtaPollConfig()?.let(::toCoreOtaPollConfig)) }
        for ((key, value) in prefs.all) {
            if (key.startsWith(AUTO_RESUME_PREFIX) && value is Boolean) {
                runCatching { session.setDeviceAutoResume(key.removePrefix(AUTO_RESUME_PREFIX), value) }
            }
            if (key.startsWith(RESUME_TARGET_PREFIX) && value is String) {
                BridgethingResumeTarget.entries.firstOrNull { it.name == value }?.let { target ->
                    runCatching { session.setDeviceResumeTarget(key.removePrefix(RESUME_TARGET_PREFIX), toCoreResumeTarget(target)) }
                }
            }
        }
        val priority = prefs.getString(PROVIDER_PRIORITY_KEY, null)
            ?.split(",")
            ?.filter { it.isNotEmpty() }
            .orEmpty()
        runCatching { session.setProviderPriority(priority) }
    }

    private fun connectDevGateway(session: CompanionSession) {
        val url = BuildConfig.BRIDGETHING_DEV_GATEWAY
        if (url.isEmpty()) return
        scope.launch {
            runCatching { session.connectNetwork(url, LinkDevice(id = DEV_LANE_DEVICE_ID, name = "dev gateway")) }
                .onFailure { android.util.Log.i("bridgething.session", "dev gateway $url: ${it.message}") }
        }
    }

    private fun loadCapabilityFlags(): BridgethingCapabilityFlags = capabilityFlags(context)

    private fun saveCapabilityFlags(f: BridgethingCapabilityFlags) {
        prefs.edit()
            .putBoolean("caps.configured", true)
            .putBoolean("caps.geo", f.geo)
            .putBoolean("caps.notifications", f.notifications)
            .putBoolean("caps.netFetch", f.netFetch)
            .putBoolean("caps.netWs", f.netWs)
            .putBoolean("caps.audioTts", f.audioTts)
            .putBoolean(VOICE_MODEL_KEY, f.voiceModel)
            .apply()
    }

    private fun loadOtaPollConfig(): BridgethingOtaPollConfig? {
        if (!prefs.getBoolean("ota.configured", false)) {
            return BridgethingOtaPollConfig(intervalSeconds = 3600.0, autoPush = true, rootUrl = null)
        }
        val root = prefs.getString("ota.rootUrl", null)
        return BridgethingOtaPollConfig(
            intervalSeconds = prefs.getLong("ota.intervalSeconds", 3600L).toDouble(),
            autoPush = prefs.getBoolean("ota.autoPush", true),
            rootUrl = if (root.isNullOrEmpty()) null else root,
        )
    }

    private fun saveOtaPollConfig(config: BridgethingOtaPollConfig?) {
        if (config == null) {
            prefs.edit().putBoolean("ota.configured", false).apply()
            return
        }
        prefs.edit()
            .putBoolean("ota.configured", true)
            .putLong("ota.intervalSeconds", config.intervalSeconds.toLong())
            .putBoolean("ota.autoPush", config.autoPush)
            .putString("ota.rootUrl", config.rootUrl)
            .apply()
    }

    private inline fun safeEmit(block: () -> Unit) {
        try {
            block()
        } catch (e: CancellationException) {
            throw e
        } catch (t: Throwable) {
            // dropped: stale callback from a torn-down runtime
        }
    }
}
