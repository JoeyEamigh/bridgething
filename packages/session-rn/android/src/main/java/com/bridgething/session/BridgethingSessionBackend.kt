package com.bridgething.session

import com.margelo.nitro.bridgething.session.BridgethingActiveWebapp
import com.margelo.nitro.bridgething.session.BridgethingAncsAuthStatus
import com.margelo.nitro.bridgething.session.BridgethingAncsSetupResult
import com.margelo.nitro.bridgething.session.BridgethingAuthState
import com.margelo.nitro.bridgething.session.BridgethingBtDevice
import com.margelo.nitro.bridgething.session.BridgethingCapabilityFlags
import com.margelo.nitro.bridgething.session.BridgethingCompanionDebug
import com.margelo.nitro.bridgething.session.BridgethingConfigEntry
import com.margelo.nitro.bridgething.session.BridgethingDeviceLogLine
import com.margelo.nitro.bridgething.session.BridgethingDeviceMeta
import com.margelo.nitro.bridgething.session.BridgethingLogArchive
import com.margelo.nitro.bridgething.session.BridgethingDocEntry
import com.margelo.nitro.bridgething.session.BridgethingNowPlaying
import com.margelo.nitro.bridgething.session.BridgethingDeviceWebappsEntry
import com.margelo.nitro.bridgething.session.BridgethingOtaAvailable
import com.margelo.nitro.bridgething.session.BridgethingOtaPollStatus
import com.margelo.nitro.bridgething.session.BridgethingOtaProgress
import com.margelo.nitro.bridgething.session.BridgethingOtaRun
import com.margelo.nitro.bridgething.session.BridgethingOtaManifest
import com.margelo.nitro.bridgething.session.BridgethingOtaPollConfig
import com.margelo.nitro.bridgething.session.BridgethingProviderInfo
import com.margelo.nitro.bridgething.session.BridgethingResourceOrigin
import com.margelo.nitro.bridgething.session.BridgethingResumeTarget
import com.margelo.nitro.bridgething.session.BridgethingServiceHealth
import com.margelo.nitro.bridgething.session.BridgethingSessionPeer
import com.margelo.nitro.bridgething.session.BridgethingSessionSnapshot
import com.margelo.nitro.bridgething.session.BridgethingVoiceModelState
import com.margelo.nitro.bridgething.session.BridgethingVoiceTurn
import com.margelo.nitro.bridgething.session.BridgethingWebappIcon
import com.margelo.nitro.bridgething.session.BridgethingWebappInfo
import com.margelo.nitro.bridgething.session.BridgethingWebappSlot
import com.margelo.nitro.bridgething.session.BridgethingWebappSlots

public interface BridgethingSessionBackend {
    public suspend fun start()
    public suspend fun stop()

    public suspend fun availableProviders(): Array<BridgethingProviderInfo>
    public suspend fun connectProvider(id: String)
    public suspend fun disconnectProvider(id: String)
    public suspend fun cancelAuth(id: String)
    public suspend fun setProviderPriority(ids: Array<String>)

    public suspend fun snapshot(): BridgethingSessionSnapshot
    public suspend fun deviceLogSnapshot(limit: Double): Array<BridgethingDeviceLogLine>
    public suspend fun companionDebug(): BridgethingCompanionDebug

    public suspend fun persistedLogSize(): Double
    public suspend fun logArchives(): Array<BridgethingLogArchive>
    public suspend fun logArchiveLines(archiveId: String, limit: Double): Array<BridgethingDeviceLogLine>
    public suspend fun exportLogs(archiveId: String?): String
    public suspend fun shareLogs(archiveId: String?): Boolean
    public suspend fun deleteLogArchive(archiveId: String)
    public suspend fun clearPersistedLogs()

    public suspend fun enableAncsNotifications(deviceId: String): BridgethingAncsSetupResult
    public suspend fun ancsAuthStatus(deviceId: String): BridgethingAncsAuthStatus

    public suspend fun listWebapps(deviceId: String): Array<BridgethingWebappInfo>
    public suspend fun currentWebapp(deviceId: String): BridgethingActiveWebapp?
    public suspend fun installWebapp(deviceId: String, sourceUri: String): BridgethingWebappInfo
    public suspend fun uninstallWebapp(deviceId: String, id: String)
    public suspend fun switchWebapp(deviceId: String, id: String)
    public suspend fun getWebappSlots(deviceId: String): BridgethingWebappSlots
    public suspend fun setWebappSlot(deviceId: String, slot: BridgethingWebappSlot, id: String?): BridgethingWebappSlots
    public suspend fun webappIcon(deviceId: String, id: String): BridgethingWebappIcon?
    public suspend fun webappSettingsMarkup(
        deviceId: String,
        id: String,
        origin: BridgethingResourceOrigin?,
    ): String
    public suspend fun listWebappConfig(deviceId: String, id: String): Array<BridgethingConfigEntry>
    public suspend fun setWebappConfigField(deviceId: String, id: String, key: String, value: String)
    public suspend fun deleteWebappConfigField(deviceId: String, id: String, key: String)
    public suspend fun getWebappDoc(deviceId: String, id: String, key: String): String?
    public suspend fun listWebappDoc(deviceId: String, id: String): Array<BridgethingDocEntry>
    public suspend fun setWebappDoc(deviceId: String, id: String, key: String, value: String)
    public suspend fun deleteWebappDoc(deviceId: String, id: String, key: String)

    public suspend fun setCapabilityFlags(flags: BridgethingCapabilityFlags)

    public suspend fun voiceModelState(): BridgethingVoiceModelState

    public suspend fun downloadVoiceModel()

    public suspend fun setDeviceAutoResume(deviceId: String, enabled: Boolean)
    public suspend fun isDeviceAutoResumeEnabled(deviceId: String): Boolean
    public suspend fun setDeviceResumeTarget(deviceId: String, target: BridgethingResumeTarget)
    public suspend fun deviceResumeTarget(deviceId: String): BridgethingResumeTarget

    public suspend fun setOtaPollConfig(config: BridgethingOtaPollConfig?)
    public suspend fun checkForOtaUpdate(rootUrl: String)
    public suspend fun fetchOtaManifest(rootUrl: String): BridgethingOtaManifest
    public suspend fun applyOtaUpdate(deviceId: String, channel: String, version: String, rootUrl: String)
    public fun otaRunProgress(deviceId: String, nowMs: Double): BridgethingOtaProgress?

    public suspend fun dismissOtaRun(deviceId: String)

    public suspend fun installWebappFromUrl(
        deviceId: String,
        url: String,
        sha256: String,
        size: Double,
        provenance: String?,
        webappId: String?,
        webappName: String?,
    ): BridgethingWebappInfo

    public suspend fun reconnectPeer(deviceId: String)

    public suspend fun deviceSetNickname(deviceId: String, nickname: String)

    public suspend fun presentPairPicker(): BridgethingBtDevice?

    public suspend fun isNotificationAccessGranted(): Boolean
    public suspend fun requestNotificationAccess()

    public suspend fun isDefaultDialer(): Boolean
    public suspend fun requestDefaultDialer()
    public suspend fun installCompanionUpdate(url: String, filename: String, size: Double, sha256: String)

    public suspend fun forgetCompanionDevice(mac: String)

    public suspend fun isIgnoringBatteryOptimizations(): Boolean
    public suspend fun requestIgnoreBatteryOptimizations()

    public suspend fun revokeRuntimePermissions(permissions: Array<String>): Boolean
    public suspend fun killApp()

    public fun setOnProvidersChanged(callback: (Array<BridgethingProviderInfo>) -> Unit)
    public fun setOnPeerConnected(callback: (BridgethingSessionPeer) -> Unit)
    public fun setOnPeerDisconnected(callback: (String) -> Unit)
    public fun setOnPeerLinkFailed(callback: (BridgethingSessionPeer) -> Unit)
    public fun setOnNowPlayingChanged(callback: (BridgethingNowPlaying?) -> Unit)
    public fun setOnAncsAuthStatusChanged(callback: (String, BridgethingAncsAuthStatus) -> Unit)
    public fun setOnLog(callback: (String, String, String) -> Unit)
    public fun setLogStreamingEnabled(enabled: Boolean)
    public fun setLocalLogStreamingEnabled(enabled: Boolean)
    public fun setOnWebappsChanged(callback: (BridgethingDeviceWebappsEntry) -> Unit)
    public fun setOnWebappDocChanged(callback: (String, String, String, String?) -> Unit)
    public fun setOnDeviceMetaChanged(callback: (String, BridgethingDeviceMeta) -> Unit)
    public fun setOnVoiceModelStateChanged(callback: (BridgethingVoiceModelState) -> Unit)

    public fun setOnVoiceTurnChanged(callback: (BridgethingVoiceTurn) -> Unit)
    public fun setOnOtaRunChanged(callback: (BridgethingOtaRun) -> Unit)

    public fun setOnOtaAvailableChanged(callback: (BridgethingOtaAvailable) -> Unit)

    public fun setOnOtaPollChanged(callback: (BridgethingOtaPollStatus) -> Unit)
    public fun setOnCompanionUpdateProgress(callback: (Double, Double) -> Unit)

    public fun setOnResumed(callback: (BridgethingSessionSnapshot) -> Unit)
}
