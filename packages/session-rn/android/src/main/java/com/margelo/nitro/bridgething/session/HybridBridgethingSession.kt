package com.margelo.nitro.bridgething.session

import com.bridgething.session.BridgethingSessionBackend
import com.facebook.proguard.annotations.DoNotStrip
import com.margelo.nitro.core.NullType
import com.margelo.nitro.core.Promise

@DoNotStrip
public class HybridBridgethingSession : HybridBridgethingSessionSpec() {

    public companion object {
        private val stateLock = Any()

        @Volatile
        private var backend: BridgethingSessionBackend? = null

        private var pendingProvidersChanged: ((Array<BridgethingProviderInfo>) -> Unit)? = null
        private var pendingPeerConnected: ((BridgethingSessionPeer) -> Unit)? = null
        private var pendingPeerDisconnected: ((String) -> Unit)? = null
        private var pendingPeerLinkFailed: ((BridgethingSessionPeer) -> Unit)? = null
        private var pendingNowPlayingChanged: ((BridgethingNowPlaying?) -> Unit)? = null
        private var pendingAncsAuthStatusChanged: ((String, BridgethingAncsAuthStatus) -> Unit)? = null
        private var pendingLog: ((String, String, String) -> Unit)? = null
        private var pendingWebappsChanged: ((BridgethingDeviceWebappsEntry) -> Unit)? = null
        private var pendingWebappDocChanged: ((String, String, String, String?) -> Unit)? = null
        private var pendingDeviceMetaChanged: ((String, BridgethingDeviceMeta) -> Unit)? = null
        private var pendingVoiceModelStateChanged: ((BridgethingVoiceModelState) -> Unit)? = null
        private var pendingVoiceTurnChanged: ((BridgethingVoiceTurn) -> Unit)? = null
        private var pendingOtaRunChanged: ((BridgethingOtaRun) -> Unit)? = null
        private var pendingOtaAvailableChanged: ((BridgethingOtaAvailable) -> Unit)? = null
        private var pendingOtaPollChanged: ((BridgethingOtaPollStatus) -> Unit)? = null
        private var pendingCompanionUpdateProgress: ((Double, Double) -> Unit)? = null
        private var pendingResumed: ((BridgethingSessionSnapshot) -> Unit)? = null

        @JvmStatic
        public fun installBackend(b: BridgethingSessionBackend) {
            val replay = synchronized(stateLock) {
                backend = b
                val snapshot = Replay(
                    providers = pendingProvidersChanged,
                    peerConnected = pendingPeerConnected,
                    peerDisconnected = pendingPeerDisconnected,
                    peerLinkFailed = pendingPeerLinkFailed,
                    nowPlaying = pendingNowPlayingChanged,
                    ancs = pendingAncsAuthStatusChanged,
                    log = pendingLog,
                    webapps = pendingWebappsChanged,
                    webappDoc = pendingWebappDocChanged,
                    deviceMeta = pendingDeviceMetaChanged,
                    voiceModel = pendingVoiceModelStateChanged,
                    voiceTurn = pendingVoiceTurnChanged,
                    otaRun = pendingOtaRunChanged,
                    otaAvailable = pendingOtaAvailableChanged,
                    otaPoll = pendingOtaPollChanged,
                    companionUpdateProgress = pendingCompanionUpdateProgress,
                    resumed = pendingResumed,
                )
                pendingProvidersChanged = null
                pendingPeerConnected = null
                pendingPeerDisconnected = null
                pendingPeerLinkFailed = null
                pendingNowPlayingChanged = null
                pendingAncsAuthStatusChanged = null
                pendingLog = null
                pendingWebappsChanged = null
                pendingWebappDocChanged = null
                pendingDeviceMetaChanged = null
                pendingVoiceModelStateChanged = null
                pendingVoiceTurnChanged = null
                pendingOtaRunChanged = null
                pendingOtaAvailableChanged = null
                pendingOtaPollChanged = null
                pendingCompanionUpdateProgress = null
                pendingResumed = null
                snapshot
            }
            replay.providers?.let(b::setOnProvidersChanged)
            replay.peerConnected?.let(b::setOnPeerConnected)
            replay.peerDisconnected?.let(b::setOnPeerDisconnected)
            replay.peerLinkFailed?.let(b::setOnPeerLinkFailed)
            replay.nowPlaying?.let(b::setOnNowPlayingChanged)
            replay.ancs?.let(b::setOnAncsAuthStatusChanged)
            replay.log?.let(b::setOnLog)
            replay.webapps?.let(b::setOnWebappsChanged)
            replay.webappDoc?.let(b::setOnWebappDocChanged)
            replay.deviceMeta?.let(b::setOnDeviceMetaChanged)
            replay.voiceModel?.let(b::setOnVoiceModelStateChanged)
            replay.voiceTurn?.let(b::setOnVoiceTurnChanged)
            replay.otaRun?.let(b::setOnOtaRunChanged)
            replay.otaAvailable?.let(b::setOnOtaAvailableChanged)
            replay.otaPoll?.let(b::setOnOtaPollChanged)
            replay.companionUpdateProgress?.let(b::setOnCompanionUpdateProgress)
            replay.resumed?.let(b::setOnResumed)
        }

        private fun require(): BridgethingSessionBackend = backend
            ?: throw RuntimeException(
                "BridgethingSession backend not installed - host app must call " +
                    "HybridBridgethingSession.installBackend(...) before React Native starts"
            )
    }

    private data class Replay(
        val providers: ((Array<BridgethingProviderInfo>) -> Unit)?,
        val peerConnected: ((BridgethingSessionPeer) -> Unit)?,
        val peerDisconnected: ((String) -> Unit)?,
        val peerLinkFailed: ((BridgethingSessionPeer) -> Unit)?,
        val nowPlaying: ((BridgethingNowPlaying?) -> Unit)?,
        val ancs: ((String, BridgethingAncsAuthStatus) -> Unit)?,
        val log: ((String, String, String) -> Unit)?,
        val webapps: ((BridgethingDeviceWebappsEntry) -> Unit)?,
        val webappDoc: ((String, String, String, String?) -> Unit)?,
        val deviceMeta: ((String, BridgethingDeviceMeta) -> Unit)?,
        val voiceModel: ((BridgethingVoiceModelState) -> Unit)?,
        val voiceTurn: ((BridgethingVoiceTurn) -> Unit)?,
        val otaRun: ((BridgethingOtaRun) -> Unit)?,
        val otaAvailable: ((BridgethingOtaAvailable) -> Unit)?,
        val otaPoll: ((BridgethingOtaPollStatus) -> Unit)?,
        val companionUpdateProgress: ((Double, Double) -> Unit)?,
        val resumed: ((BridgethingSessionSnapshot) -> Unit)?,
    )

    override fun start(): Promise<Unit> = Promise.async { require().start() }
    override fun stop(): Promise<Unit> = Promise.async { require().stop() }

    override fun availableProviders(): Promise<Array<BridgethingProviderInfo>> = Promise.async {
        require().availableProviders()
    }

    override fun connectProvider(id: String): Promise<Unit> = Promise.async { require().connectProvider(id) }
    override fun disconnectProvider(id: String): Promise<Unit> = Promise.async { backend?.disconnectProvider(id) }
    override fun cancelAuth(id: String): Promise<Unit> = Promise.async { backend?.cancelAuth(id) }
    override fun setProviderPriority(ids: Array<String>): Promise<Unit> = Promise.async { backend?.setProviderPriority(ids) }

    override fun snapshot(): Promise<BridgethingSessionSnapshot> = Promise.async {
        require().snapshot()
    }

    override fun deviceLogSnapshot(limit: Double): Promise<Array<BridgethingDeviceLogLine>> = Promise.async {
        backend?.deviceLogSnapshot(limit) ?: emptyArray()
    }

    override fun companionDebug(): Promise<BridgethingCompanionDebug> = Promise.async {
        require().companionDebug()
    }

    override fun persistedLogSize(): Promise<Double> = Promise.async {
        backend?.persistedLogSize() ?: 0.0
    }

    override fun logArchives(): Promise<Array<BridgethingLogArchive>> = Promise.async {
        backend?.logArchives() ?: emptyArray()
    }

    override fun logArchiveLines(archiveId: String, limit: Double): Promise<Array<BridgethingDeviceLogLine>> =
        Promise.async { backend?.logArchiveLines(archiveId, limit) ?: emptyArray() }

    override fun exportLogs(archiveId: Variant_NullType_String?): Promise<String> = Promise.async {
        require().exportLogs(unwrapString(archiveId))
    }

    override fun shareLogs(archiveId: Variant_NullType_String?): Promise<Boolean> = Promise.async {
        backend?.shareLogs(unwrapString(archiveId)) ?: false
    }

    override fun deleteLogArchive(archiveId: String): Promise<Unit> = Promise.async {
        backend?.deleteLogArchive(archiveId)
    }

    override fun clearPersistedLogs(): Promise<Unit> = Promise.async { backend?.clearPersistedLogs() }

    override fun enableAncsNotifications(deviceId: String): Promise<BridgethingAncsSetupResult> = Promise.async {
        backend?.enableAncsNotifications(deviceId) ?: BridgethingAncsSetupResult(
            kind = BridgethingAncsSetupKind.UNSUPPORTED,
            authStatus = BridgethingAncsAuthStatus.UNKNOWN,
            message = null,
        )
    }

    override fun ancsAuthStatus(deviceId: String): Promise<BridgethingAncsAuthStatus> = Promise.async {
        backend?.ancsAuthStatus(deviceId) ?: BridgethingAncsAuthStatus.UNKNOWN
    }

    override fun listWebapps(deviceId: String): Promise<Array<BridgethingWebappInfo>> = Promise.async {
        require().listWebapps(deviceId)
    }

    override fun currentWebapp(deviceId: String): Promise<Variant_NullType_BridgethingActiveWebapp> = Promise.async {
        val active = require().currentWebapp(deviceId)
        if (active != null) Variant_NullType_BridgethingActiveWebapp.Second(active)
        else Variant_NullType_BridgethingActiveWebapp.First(NullType.NULL)
    }

    override fun installWebapp(deviceId: String, sourceUri: String): Promise<BridgethingWebappInfo> = Promise.async {
        require().installWebapp(deviceId, sourceUri)
    }

    override fun uninstallWebapp(deviceId: String, id: String): Promise<Unit> = Promise.async {
        require().uninstallWebapp(deviceId, id)
    }

    override fun switchWebapp(deviceId: String, id: String): Promise<Unit> = Promise.async {
        require().switchWebapp(deviceId, id)
    }

    override fun getWebappSlots(deviceId: String): Promise<BridgethingWebappSlots> = Promise.async {
        require().getWebappSlots(deviceId)
    }

    override fun setWebappSlot(deviceId: String, slot: BridgethingWebappSlot, id: String?): Promise<BridgethingWebappSlots> = Promise.async {
        require().setWebappSlot(deviceId, slot, id)
    }

    override fun webappIcon(deviceId: String, id: String): Promise<Variant_NullType_BridgethingWebappIcon> = Promise.async {
        val icon = require().webappIcon(deviceId, id)
        if (icon != null) Variant_NullType_BridgethingWebappIcon.Second(icon)
        else Variant_NullType_BridgethingWebappIcon.First(NullType.NULL)
    }

    override fun webappSettingsMarkup(
        deviceId: String,
        id: String,
        origin: BridgethingResourceOrigin?,
    ): Promise<String> = Promise.async {
        require().webappSettingsMarkup(deviceId, id, origin)
    }

    override fun listWebappConfig(deviceId: String, id: String): Promise<Array<BridgethingConfigEntry>> = Promise.async {
        require().listWebappConfig(deviceId, id)
    }

    override fun setWebappConfigField(deviceId: String, id: String, key: String, value: String): Promise<Unit> = Promise.async {
        require().setWebappConfigField(deviceId, id, key, value)
    }

    override fun deleteWebappConfigField(deviceId: String, id: String, key: String): Promise<Unit> = Promise.async {
        require().deleteWebappConfigField(deviceId, id, key)
    }

    override fun getWebappDoc(deviceId: String, id: String, key: String): Promise<Variant_NullType_String> = Promise.async {
        val value = require().getWebappDoc(deviceId, id, key)
        if (value != null) Variant_NullType_String.Second(value)
        else Variant_NullType_String.First(NullType.NULL)
    }

    override fun listWebappDoc(deviceId: String, id: String): Promise<Array<BridgethingDocEntry>> = Promise.async {
        require().listWebappDoc(deviceId, id)
    }

    override fun setWebappDoc(deviceId: String, id: String, key: String, value: String): Promise<Unit> = Promise.async {
        require().setWebappDoc(deviceId, id, key, value)
    }

    override fun deleteWebappDoc(deviceId: String, id: String, key: String): Promise<Unit> = Promise.async {
        require().deleteWebappDoc(deviceId, id, key)
    }

    override fun setCapabilityFlags(flags: BridgethingCapabilityFlags): Promise<Unit> = Promise.async {
        backend?.setCapabilityFlags(flags)
    }

    override fun voiceModelState(): Promise<BridgethingVoiceModelState> = Promise.async {
        require().voiceModelState()
    }

    override fun downloadVoiceModel(): Promise<Unit> = Promise.async {
        backend?.downloadVoiceModel()
    }

    override fun setDeviceAutoResume(deviceId: String, enabled: Boolean): Promise<Unit> = Promise.async {
        backend?.setDeviceAutoResume(deviceId, enabled)
    }

    override fun isDeviceAutoResumeEnabled(deviceId: String): Promise<Boolean> = Promise.async {
        backend?.isDeviceAutoResumeEnabled(deviceId) ?: true
    }

    override fun setDeviceResumeTarget(deviceId: String, target: BridgethingResumeTarget): Promise<Unit> = Promise.async {
        backend?.setDeviceResumeTarget(deviceId, target)
    }

    override fun deviceResumeTarget(deviceId: String): Promise<BridgethingResumeTarget> = Promise.async {
        backend?.deviceResumeTarget(deviceId) ?: BridgethingResumeTarget.PHONEONLY
    }

    override fun setOtaPollConfig(config: Variant_NullType_BridgethingOtaPollConfig?): Promise<Unit> = Promise.async {
        val unwrapped: BridgethingOtaPollConfig? = config?.let { variant ->
            when (variant) {
                is Variant_NullType_BridgethingOtaPollConfig.First -> null
                is Variant_NullType_BridgethingOtaPollConfig.Second -> variant.value
            }
        }
        backend?.setOtaPollConfig(unwrapped)
    }

    override fun checkForOtaUpdate(rootUrl: String): Promise<Unit> = Promise.async {
        backend?.checkForOtaUpdate(rootUrl)
    }

    override fun fetchOtaManifest(rootUrl: String): Promise<BridgethingOtaManifest> = Promise.async {
        require().fetchOtaManifest(rootUrl)
    }

    override fun dismissOtaRun(deviceId: String): Promise<Unit> = Promise.async {
        require().dismissOtaRun(deviceId)
    }

    override fun applyOtaUpdate(deviceId: String, channel: String, version: String, rootUrl: String): Promise<Unit> = Promise.async {
        backend?.applyOtaUpdate(deviceId, channel, version, rootUrl)
    }

    override fun otaRunProgress(deviceId: String, nowMs: Double): Variant_NullType_BridgethingOtaProgress {
        val progress = backend?.otaRunProgress(deviceId, nowMs)
        return if (progress != null) Variant_NullType_BridgethingOtaProgress.Second(progress)
        else Variant_NullType_BridgethingOtaProgress.First(NullType.NULL)
    }

    override fun installWebappFromUrl(
        deviceId: String,
        url: String,
        sha256: String,
        size: Double,
        provenance: Variant_NullType_String?,
        webappId: Variant_NullType_String?,
        webappName: Variant_NullType_String?,
    ): Promise<BridgethingWebappInfo> = Promise.async {
        require().installWebappFromUrl(
            deviceId, url, sha256, size,
            unwrapString(provenance), unwrapString(webappId), unwrapString(webappName),
        )
    }

    override fun reconnectPeer(deviceId: String): Promise<Unit> = Promise.async {
        backend?.reconnectPeer(deviceId)
    }

    override fun deviceSetNickname(deviceId: String, nickname: String): Promise<Unit> = Promise.async {
        require().deviceSetNickname(deviceId, nickname)
    }

    override fun presentPairPicker(): Promise<Variant_NullType_BridgethingBtDevice> = Promise.async {
        val device = require().presentPairPicker()
        if (device != null) Variant_NullType_BridgethingBtDevice.Second(device)
        else Variant_NullType_BridgethingBtDevice.First(NullType.NULL)
    }

    override fun isNotificationAccessGranted(): Promise<Boolean> = Promise.async {
        backend?.isNotificationAccessGranted() ?: false
    }

    override fun requestNotificationAccess(): Promise<Unit> = Promise.async {
        require().requestNotificationAccess()
    }

    override fun isDefaultDialer(): Promise<Boolean> = Promise.async {
        backend?.isDefaultDialer() ?: false
    }

    override fun requestDefaultDialer(): Promise<Unit> = Promise.async {
        require().requestDefaultDialer()
    }

    override fun installCompanionUpdate(url: String, filename: String, size: Double, sha256: String): Promise<Unit> = Promise.async {
        require().installCompanionUpdate(url, filename, size, sha256)
    }

    override fun forgetCompanionDevice(mac: String): Promise<Unit> = Promise.async {
        require().forgetCompanionDevice(mac)
    }

    override fun isIgnoringBatteryOptimizations(): Promise<Boolean> = Promise.async {
        backend?.isIgnoringBatteryOptimizations() ?: false
    }

    override fun requestIgnoreBatteryOptimizations(): Promise<Unit> = Promise.async {
        require().requestIgnoreBatteryOptimizations()
    }

    override fun revokeRuntimePermissions(permissions: Array<String>): Promise<Boolean> = Promise.async {
        backend?.revokeRuntimePermissions(permissions) ?: false
    }

    override fun killApp(): Promise<Unit> = Promise.async {
        backend?.killApp()
    }

    override fun setOnProvidersChanged(callback: (providers: Array<BridgethingProviderInfo>) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnProvidersChanged) { pendingProvidersChanged = it }
    }

    override fun setOnPeerConnected(callback: (peer: BridgethingSessionPeer) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnPeerConnected) { pendingPeerConnected = it }
    }

    override fun setOnPeerDisconnected(callback: (peerId: String) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnPeerDisconnected) { pendingPeerDisconnected = it }
    }

    override fun setOnPeerLinkFailed(callback: (peer: BridgethingSessionPeer) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnPeerLinkFailed) { pendingPeerLinkFailed = it }
    }

    override fun setOnNowPlayingChanged(callback: (now: Variant_NullType_BridgethingNowPlaying?) -> Unit) {
        val wrapped: (BridgethingNowPlaying?) -> Unit = { np ->
            val variant = if (np != null) Variant_NullType_BridgethingNowPlaying.Second(np)
            else Variant_NullType_BridgethingNowPlaying.First(NullType.NULL)
            callback(variant)
        }
        forwardOrBuffer(wrapped, BridgethingSessionBackend::setOnNowPlayingChanged) { pendingNowPlayingChanged = it }
    }

    override fun setOnAncsAuthStatusChanged(callback: (deviceId: String, status: BridgethingAncsAuthStatus) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnAncsAuthStatusChanged) { pendingAncsAuthStatusChanged = it }
    }

    override fun setOnLog(callback: (origin: String, level: String, message: String) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnLog) { pendingLog = it }
    }

    override fun setLogStreamingEnabled(enabled: Boolean) {
        backend?.setLogStreamingEnabled(enabled)
    }

    override fun setLocalLogStreamingEnabled(enabled: Boolean) {
        backend?.setLocalLogStreamingEnabled(enabled)
    }

    override fun setOnWebappsChanged(callback: (entry: BridgethingDeviceWebappsEntry) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnWebappsChanged) { pendingWebappsChanged = it }
    }

    override fun setOnWebappDocChanged(
        callback: (deviceId: String, webappId: String, key: String, value: Variant_NullType_String?) -> Unit,
    ) {
        val wrapped: (String, String, String, String?) -> Unit = { deviceId, webappId, key, value ->
            val variant =
                if (value != null) Variant_NullType_String.Second(value)
                else Variant_NullType_String.First(NullType.NULL)
            callback(deviceId, webappId, key, variant)
        }
        forwardOrBuffer(wrapped, BridgethingSessionBackend::setOnWebappDocChanged) { pendingWebappDocChanged = it }
    }

    override fun setOnDeviceMetaChanged(callback: (deviceId: String, meta: BridgethingDeviceMeta) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnDeviceMetaChanged) { pendingDeviceMetaChanged = it }
    }

    override fun setOnVoiceModelStateChanged(callback: (state: BridgethingVoiceModelState) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnVoiceModelStateChanged) {
            pendingVoiceModelStateChanged = it
        }
    }

    override fun setOnVoiceTurnChanged(callback: (turn: BridgethingVoiceTurn) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnVoiceTurnChanged) { pendingVoiceTurnChanged = it }
    }

    override fun setOnOtaRunChanged(callback: (run: BridgethingOtaRun) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnOtaRunChanged) { pendingOtaRunChanged = it }
    }

    override fun setOnOtaAvailableChanged(callback: (available: BridgethingOtaAvailable) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnOtaAvailableChanged) { pendingOtaAvailableChanged = it }
    }

    override fun setOnOtaPollChanged(callback: (status: BridgethingOtaPollStatus) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnOtaPollChanged) { pendingOtaPollChanged = it }
    }

    override fun setOnCompanionUpdateProgress(callback: (received: Double, total: Double) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnCompanionUpdateProgress) { pendingCompanionUpdateProgress = it }
    }

    override fun setOnResumed(callback: (snapshot: BridgethingSessionSnapshot) -> Unit) {
        forwardOrBuffer(callback, BridgethingSessionBackend::setOnResumed) { pendingResumed = it }
    }

    private fun unwrapString(variant: Variant_NullType_String?): String? = variant?.let {
        when (it) {
            is Variant_NullType_String.First -> null
            is Variant_NullType_String.Second -> it.value
        }
    }

    private inline fun <C> forwardOrBuffer(
        callback: C,
        forward: BridgethingSessionBackend.(C) -> Unit,
        buffer: (C) -> Unit,
    ) {
        val current = synchronized(stateLock) {
            val b = backend
            if (b == null) buffer(callback)
            b
        }
        if (current != null) current.forward(callback)
    }
}
