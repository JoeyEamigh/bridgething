import Foundation
import NitroModules

public protocol BridgethingSessionBackend: AnyObject, Sendable {
    func start() async throws
    func stop() async

    func availableProviders() async -> [BridgethingProviderInfo]
    func connectProvider(id: String) async throws
    func disconnectProvider(id: String) async
    func cancelAuth(id: String) async
    func setProviderPriority(ids: [String]) async

    func snapshot() async -> BridgethingSessionSnapshot
    func deviceLogSnapshot(limit: Double) async -> [BridgethingDeviceLogLine]
    func companionDebug() async throws -> BridgethingCompanionDebug

    func persistedLogSize() async -> Double
    func logArchives() async -> [BridgethingLogArchive]
    func logArchiveLines(archiveId: String, limit: Double) async -> [BridgethingDeviceLogLine]
    func exportLogs(archiveId: String?) async throws -> String
    func shareLogs(archiveId: String?) async -> Bool
    func deleteLogArchive(archiveId: String) async
    func clearPersistedLogs() async

    func enableAncsNotifications(deviceId: String) async -> BridgethingAncsSetupResult
    func ancsAuthStatus(deviceId: String) async -> BridgethingAncsAuthStatus

    func listWebapps(deviceId: String) async throws -> [BridgethingWebappInfo]
    func currentWebapp(deviceId: String) async throws -> BridgethingActiveWebapp?
    func installWebapp(deviceId: String, sourceUri: String) async throws -> BridgethingWebappInfo
    func uninstallWebapp(deviceId: String, id: String) async throws
    func switchWebapp(deviceId: String, id: String) async throws
    func getWebappSlots(deviceId: String) async throws -> BridgethingWebappSlots
    func setWebappSlot(deviceId: String, slot: BridgethingWebappSlot, id: String?) async throws
        -> BridgethingWebappSlots
    func webappIcon(deviceId: String, id: String) async throws -> BridgethingWebappIcon?
    func webappSettingsPage(deviceId: String, id: String) async throws -> String
    func listWebappConfig(deviceId: String, id: String) async throws -> [BridgethingConfigEntry]
    func setWebappConfigField(deviceId: String, id: String, key: String, value: String) async throws
    func deleteWebappConfigField(deviceId: String, id: String, key: String) async throws
    func getWebappDoc(deviceId: String, id: String, key: String) async throws -> String?
    func listWebappDoc(deviceId: String, id: String) async throws -> [BridgethingDocEntry]
    func setWebappDoc(deviceId: String, id: String, key: String, value: String) async throws
    func deleteWebappDoc(deviceId: String, id: String, key: String) async throws

    func setCapabilityFlags(flags: BridgethingCapabilityFlags) async

    func voiceModelState() async -> BridgethingVoiceModelState
    func downloadVoiceModel() async

    func setDeviceAutoResume(deviceId: String, enabled: Bool) async
    func isDeviceAutoResumeEnabled(deviceId: String) async -> Bool
    func setDeviceResumeTarget(deviceId: String, target: BridgethingResumeTarget) async
    func deviceResumeTarget(deviceId: String) async -> BridgethingResumeTarget

    func setOtaPollConfig(config: BridgethingOtaPollConfig?) async
    func checkForOtaUpdate(rootUrl: String) async
    func fetchOtaManifest(rootUrl: String) async throws -> BridgethingOtaManifest
    func applyOtaUpdate(deviceId: String, channel: String, version: String, rootUrl: String) async throws
    func otaRunProgress(deviceId: String, nowMs: Double) -> BridgethingOtaProgress?
    func dismissOtaRun(deviceId: String) async throws

    func installWebappFromUrl(
        deviceId: String,
        url: String,
        sha256: String,
        size: Double,
        provenance: String?,
        webappId: String?,
        webappName: String?
    ) async throws -> BridgethingWebappInfo

    func reconnectPeer(deviceId: String) async throws

    func deviceSetNickname(deviceId: String, nickname: String) async throws

    func presentPairPicker() async throws -> BridgethingBtDevice?

    func isNotificationAccessGranted() async -> Bool
    func requestNotificationAccess() async throws

    func isDefaultDialer() async -> Bool
    func requestDefaultDialer() async throws

    func forgetCompanionDevice(mac: String) async throws

    func isIgnoringBatteryOptimizations() async -> Bool
    func requestIgnoreBatteryOptimizations() async throws

    func revokeRuntimePermissions(permissions: [String]) async -> Bool
    func killApp() async

    func setOnProvidersChanged(_ callback: @escaping @Sendable ([BridgethingProviderInfo]) -> Void)
    func setOnPeerConnected(_ callback: @escaping @Sendable (BridgethingSessionPeer) -> Void)
    func setOnPeerDisconnected(_ callback: @escaping @Sendable (String) -> Void)
    func setOnPeerLinkFailed(_ callback: @escaping @Sendable (BridgethingSessionPeer) -> Void)
    func setOnNowPlayingChanged(_ callback: @escaping @Sendable (BridgethingNowPlaying?) -> Void)
    func setOnAncsAuthStatusChanged(_ callback: @escaping @Sendable (String, BridgethingAncsAuthStatus) -> Void)
    func setOnLog(_ callback: @escaping @Sendable (String, String, String) -> Void)
    func setLogStreamingEnabled(_ enabled: Bool)
    func setLocalLogStreamingEnabled(_ enabled: Bool)

    func setOnWebappsChanged(_ callback: @escaping @Sendable (BridgethingDeviceWebappsEntry) -> Void)
    func setOnWebappDocChanged(_ callback: @escaping @Sendable (String, String, String, String?) -> Void)
    func setOnDeviceMetaChanged(_ callback: @escaping @Sendable (String, BridgethingDeviceMeta) -> Void)
    func setOnVoiceModelStateChanged(_ callback: @escaping @Sendable (BridgethingVoiceModelState) -> Void)
    func setOnVoiceTurnChanged(_ callback: @escaping @Sendable (BridgethingVoiceTurn) -> Void)
    func setOnOtaRunChanged(_ callback: @escaping @Sendable (BridgethingOtaRun) -> Void)
    func setOnOtaAvailableChanged(_ callback: @escaping @Sendable (BridgethingOtaAvailable) -> Void)
    func setOnOtaPollChanged(_ callback: @escaping @Sendable (BridgethingOtaPollStatus) -> Void)
    func setOnResumed(_ callback: @escaping @Sendable (BridgethingSessionSnapshot) -> Void)
}

public final class HybridBridgethingSession: HybridBridgethingSessionSpec, @unchecked Sendable {
    private static let stateLock = NSLock()
    private static var _backend: (any BridgethingSessionBackend)?

    private static var pendingProvidersChanged: (@Sendable ([BridgethingProviderInfo]) -> Void)?
    private static var pendingPeerConnected: (@Sendable (BridgethingSessionPeer) -> Void)?
    private static var pendingPeerDisconnected: (@Sendable (String) -> Void)?
    private static var pendingPeerLinkFailed: (@Sendable (BridgethingSessionPeer) -> Void)?
    private static var pendingNowPlayingChanged: (@Sendable (BridgethingNowPlaying?) -> Void)?
    private static var pendingAncsAuthStatusChanged: (@Sendable (String, BridgethingAncsAuthStatus) -> Void)?
    private static var pendingLog: (@Sendable (String, String, String) -> Void)?
    private static var pendingWebappsChanged: (@Sendable (BridgethingDeviceWebappsEntry) -> Void)?
    private static var pendingWebappDocChanged: (@Sendable (String, String, String, String?) -> Void)?
    private static var pendingDeviceMetaChanged: (@Sendable (String, BridgethingDeviceMeta) -> Void)?
    private static var pendingVoiceModelStateChanged: (@Sendable (BridgethingVoiceModelState) -> Void)?
    private static var pendingVoiceTurnChanged: (@Sendable (BridgethingVoiceTurn) -> Void)?
    private static var pendingOtaRunChanged: (@Sendable (BridgethingOtaRun) -> Void)?
    private static var pendingOtaAvailableChanged: (@Sendable (BridgethingOtaAvailable) -> Void)?
    private static var pendingOtaPollChanged: (@Sendable (BridgethingOtaPollStatus) -> Void)?
    private static var pendingResumed: (@Sendable (BridgethingSessionSnapshot) -> Void)?

    public static func installBackend(_ backend: any BridgethingSessionBackend) {
        stateLock.lock()
        _backend = backend
        let providerCb = pendingProvidersChanged
        let peerConnCb = pendingPeerConnected
        let peerDisconnCb = pendingPeerDisconnected
        let peerLinkFailedCb = pendingPeerLinkFailed
        let nowPlayingCb = pendingNowPlayingChanged
        let ancsCb = pendingAncsAuthStatusChanged
        let logCb = pendingLog
        let webappsCb = pendingWebappsChanged
        let webappDocCb = pendingWebappDocChanged
        let deviceMetaCb = pendingDeviceMetaChanged
        let voiceModelCb = pendingVoiceModelStateChanged
        let voiceTurnCb = pendingVoiceTurnChanged
        let otaRunCb = pendingOtaRunChanged
        let otaAvailCb = pendingOtaAvailableChanged
        let otaPollCb = pendingOtaPollChanged
        let resumedCb = pendingResumed
        pendingProvidersChanged = nil
        pendingPeerConnected = nil
        pendingPeerDisconnected = nil
        pendingPeerLinkFailed = nil
        pendingNowPlayingChanged = nil
        pendingAncsAuthStatusChanged = nil
        pendingLog = nil
        pendingWebappsChanged = nil
        pendingWebappDocChanged = nil
        pendingDeviceMetaChanged = nil
        pendingVoiceModelStateChanged = nil
        pendingVoiceTurnChanged = nil
        pendingOtaRunChanged = nil
        pendingOtaAvailableChanged = nil
        pendingOtaPollChanged = nil
        pendingResumed = nil
        stateLock.unlock()

        if let providerCb { backend.setOnProvidersChanged(providerCb) }
        if let peerConnCb { backend.setOnPeerConnected(peerConnCb) }
        if let peerDisconnCb { backend.setOnPeerDisconnected(peerDisconnCb) }
        if let peerLinkFailedCb { backend.setOnPeerLinkFailed(peerLinkFailedCb) }
        if let nowPlayingCb { backend.setOnNowPlayingChanged(nowPlayingCb) }
        if let ancsCb { backend.setOnAncsAuthStatusChanged(ancsCb) }
        if let logCb { backend.setOnLog(logCb) }
        if let webappsCb { backend.setOnWebappsChanged(webappsCb) }
        if let webappDocCb { backend.setOnWebappDocChanged(webappDocCb) }
        if let deviceMetaCb { backend.setOnDeviceMetaChanged(deviceMetaCb) }
        if let voiceModelCb { backend.setOnVoiceModelStateChanged(voiceModelCb) }
        if let voiceTurnCb { backend.setOnVoiceTurnChanged(voiceTurnCb) }
        if let otaRunCb { backend.setOnOtaRunChanged(otaRunCb) }
        if let otaAvailCb { backend.setOnOtaAvailableChanged(otaAvailCb) }
        if let otaPollCb { backend.setOnOtaPollChanged(otaPollCb) }
        if let resumedCb { backend.setOnResumed(resumedCb) }
    }

    private static func backend() throws -> any BridgethingSessionBackend {
        stateLock.lock(); defer { stateLock.unlock() }
        guard let b = _backend else {
            throw RuntimeError.error(withMessage: "BridgethingSession backend not installed - host app must call HybridBridgethingSession.installBackend(_:) before React Native starts")
        }
        return b
    }

    private static func unwrapString(_ variant: Variant_NullType_String?) -> String? {
        variant.flatMap { v in
            switch v {
            case .first: nil
            case let .second(value): value
            }
        }
    }

    override public init() { super.init() }

    // MARK: - Lifecycle

    public func start() throws -> Promise<Void> {
        Promise.async {
            try await Self.backend().start()
        }
    }

    public func stop() throws -> Promise<Void> {
        Promise.async {
            await (try Self.backend()).stop()
        }
    }

    // MARK: - Provider selection

    public func availableProviders() throws -> Promise<[BridgethingProviderInfo]> {
        Promise.async {
            await (try Self.backend()).availableProviders()
        }
    }

    public func connectProvider(id: String) throws -> Promise<Void> {
        Promise.async {
            try await Self.backend().connectProvider(id: id)
        }
    }

    public func disconnectProvider(id: String) throws -> Promise<Void> {
        Promise.async {
            await (try Self.backend()).disconnectProvider(id: id)
        }
    }

    public func cancelAuth(id: String) throws -> Promise<Void> {
        Promise.async {
            await (try Self.backend()).cancelAuth(id: id)
        }
    }

    public func setProviderPriority(ids: [String]) throws -> Promise<Void> {
        Promise.async {
            await (try Self.backend()).setProviderPriority(ids: ids)
        }
    }

    public func snapshot() throws -> Promise<BridgethingSessionSnapshot> {
        Promise.async {
            await (try Self.backend()).snapshot()
        }
    }

    public func deviceLogSnapshot(limit: Double) throws -> Promise<[BridgethingDeviceLogLine]> {
        Promise.async {
            await (try Self.backend()).deviceLogSnapshot(limit: limit)
        }
    }

    public func companionDebug() throws -> Promise<BridgethingCompanionDebug> {
        Promise.async {
            try await (try Self.backend()).companionDebug()
        }
    }

    public func persistedLogSize() throws -> Promise<Double> {
        Promise.async {
            await (try Self.backend()).persistedLogSize()
        }
    }

    public func logArchives() throws -> Promise<[BridgethingLogArchive]> {
        Promise.async {
            await (try Self.backend()).logArchives()
        }
    }

    public func logArchiveLines(archiveId: String, limit: Double) throws -> Promise<[BridgethingDeviceLogLine]> {
        Promise.async {
            await (try Self.backend()).logArchiveLines(archiveId: archiveId, limit: limit)
        }
    }

    public func exportLogs(archiveId: Variant_NullType_String?) throws -> Promise<String> {
        Promise.async {
            try await (try Self.backend()).exportLogs(archiveId: Self.unwrapString(archiveId))
        }
    }

    public func shareLogs(archiveId: Variant_NullType_String?) throws -> Promise<Bool> {
        Promise.async {
            await (try Self.backend()).shareLogs(archiveId: Self.unwrapString(archiveId))
        }
    }

    public func deleteLogArchive(archiveId: String) throws -> Promise<Void> {
        Promise.async {
            await (try Self.backend()).deleteLogArchive(archiveId: archiveId)
        }
    }

    public func clearPersistedLogs() throws -> Promise<Void> {
        Promise.async {
            await (try Self.backend()).clearPersistedLogs()
        }
    }

    // MARK: - ANCS

    public func enableAncsNotifications(deviceId: String) throws -> Promise<BridgethingAncsSetupResult> {
        Promise.async {
            await (try Self.backend()).enableAncsNotifications(deviceId: deviceId)
        }
    }

    public func ancsAuthStatus(deviceId: String) throws -> Promise<BridgethingAncsAuthStatus> {
        Promise.async {
            await (try Self.backend()).ancsAuthStatus(deviceId: deviceId)
        }
    }

    // MARK: - Webapps (per-device)

    public func listWebapps(deviceId: String) throws -> Promise<[BridgethingWebappInfo]> {
        Promise.async {
            try await Self.backend().listWebapps(deviceId: deviceId)
        }
    }

    public func currentWebapp(deviceId: String) throws -> Promise<Variant_NullType_BridgethingActiveWebapp> {
        Promise.async {
            let active = try await Self.backend().currentWebapp(deviceId: deviceId)
            return active.map { .second($0) } ?? .first(NullType.null)
        }
    }

    public func installWebapp(deviceId: String, sourceUri: String) throws -> Promise<BridgethingWebappInfo> {
        Promise.async {
            try await Self.backend().installWebapp(deviceId: deviceId, sourceUri: sourceUri)
        }
    }

    public func uninstallWebapp(deviceId: String, id: String) throws -> Promise<Void> {
        Promise.async {
            try await Self.backend().uninstallWebapp(deviceId: deviceId, id: id)
        }
    }

    public func switchWebapp(deviceId: String, id: String) throws -> Promise<Void> {
        Promise.async {
            try await Self.backend().switchWebapp(deviceId: deviceId, id: id)
        }
    }

    public func getWebappSlots(deviceId: String) throws -> Promise<BridgethingWebappSlots> {
        Promise.async {
            try await Self.backend().getWebappSlots(deviceId: deviceId)
        }
    }

    public func setWebappSlot(deviceId: String, slot: BridgethingWebappSlot, id: String?) throws
        -> Promise<BridgethingWebappSlots>
    {
        Promise.async {
            try await Self.backend().setWebappSlot(deviceId: deviceId, slot: slot, id: id)
        }
    }

    public func webappIcon(deviceId: String, id: String) throws -> Promise<Variant_NullType_BridgethingWebappIcon> {
        Promise.async {
            let icon = try await Self.backend().webappIcon(deviceId: deviceId, id: id)
            return icon.map { .second($0) } ?? .first(NullType.null)
        }
    }

    public func webappSettingsPage(deviceId: String, id: String) throws -> Promise<String> {
        Promise.async {
            try await Self.backend().webappSettingsPage(deviceId: deviceId, id: id)
        }
    }

    public func listWebappConfig(deviceId: String, id: String) throws -> Promise<[BridgethingConfigEntry]> {
        Promise.async {
            try await Self.backend().listWebappConfig(deviceId: deviceId, id: id)
        }
    }

    public func setWebappConfigField(deviceId: String, id: String, key: String, value: String) throws -> Promise<Void> {
        Promise.async {
            try await Self.backend().setWebappConfigField(deviceId: deviceId, id: id, key: key, value: value)
        }
    }

    public func deleteWebappConfigField(deviceId: String, id: String, key: String) throws -> Promise<Void> {
        Promise.async {
            try await Self.backend().deleteWebappConfigField(deviceId: deviceId, id: id, key: key)
        }
    }

    public func getWebappDoc(deviceId: String, id: String, key: String) throws -> Promise<Variant_NullType_String> {
        Promise.async {
            let value = try await Self.backend().getWebappDoc(deviceId: deviceId, id: id, key: key)
            return value.map { .second($0) } ?? .first(NullType.null)
        }
    }

    public func listWebappDoc(deviceId: String, id: String) throws -> Promise<[BridgethingDocEntry]> {
        Promise.async {
            try await Self.backend().listWebappDoc(deviceId: deviceId, id: id)
        }
    }

    public func setWebappDoc(deviceId: String, id: String, key: String, value: String) throws -> Promise<Void> {
        Promise.async {
            try await Self.backend().setWebappDoc(deviceId: deviceId, id: id, key: key, value: value)
        }
    }

    public func deleteWebappDoc(deviceId: String, id: String, key: String) throws -> Promise<Void> {
        Promise.async {
            try await Self.backend().deleteWebappDoc(deviceId: deviceId, id: id, key: key)
        }
    }

    // MARK: - Capability flags

    public func setCapabilityFlags(flags: BridgethingCapabilityFlags) throws -> Promise<Void> {
        Promise.async {
            await (try Self.backend()).setCapabilityFlags(flags: flags)
        }
    }

    public func voiceModelState() throws -> Promise<BridgethingVoiceModelState> {
        Promise.async {
            await (try Self.backend()).voiceModelState()
        }
    }

    public func downloadVoiceModel() throws -> Promise<Void> {
        Promise.async {
            await (try Self.backend()).downloadVoiceModel()
        }
    }

    // MARK: - OTA

    public func setDeviceAutoResume(deviceId: String, enabled: Bool) throws -> Promise<Void> {
        Promise.async {
            await (try Self.backend()).setDeviceAutoResume(deviceId: deviceId, enabled: enabled)
        }
    }

    public func isDeviceAutoResumeEnabled(deviceId: String) throws -> Promise<Bool> {
        Promise.async {
            await (try Self.backend()).isDeviceAutoResumeEnabled(deviceId: deviceId)
        }
    }

    public func setDeviceResumeTarget(deviceId: String, target: BridgethingResumeTarget) throws -> Promise<Void> {
        Promise.async {
            await (try Self.backend()).setDeviceResumeTarget(deviceId: deviceId, target: target)
        }
    }

    public func deviceResumeTarget(deviceId: String) throws -> Promise<BridgethingResumeTarget> {
        Promise.async {
            await (try Self.backend()).deviceResumeTarget(deviceId: deviceId)
        }
    }

    public func setOtaPollConfig(config: Variant_NullType_BridgethingOtaPollConfig?) throws -> Promise<Void> {
        let unwrapped: BridgethingOtaPollConfig? = config.flatMap { variant in
            switch variant {
            case .first: nil
            case let .second(value): value
            }
        }
        return Promise.async {
            await (try Self.backend()).setOtaPollConfig(config: unwrapped)
        }
    }

    public func checkForOtaUpdate(rootUrl: String) throws -> Promise<Void> {
        Promise.async {
            await (try Self.backend()).checkForOtaUpdate(rootUrl: rootUrl)
        }
    }

    public func fetchOtaManifest(rootUrl: String) throws -> Promise<BridgethingOtaManifest> {
        Promise.async {
            try await Self.backend().fetchOtaManifest(rootUrl: rootUrl)
        }
    }

    public func dismissOtaRun(deviceId: String) throws -> Promise<Void> {
        Promise.async { try await Self.backend().dismissOtaRun(deviceId: deviceId) }
    }

    public func applyOtaUpdate(deviceId: String, channel: String, version: String, rootUrl: String) throws -> Promise<Void> {
        Promise.async {
            try await Self.backend().applyOtaUpdate(deviceId: deviceId, channel: channel, version: version, rootUrl: rootUrl)
        }
    }

    public func otaRunProgress(deviceId: String, nowMs: Double) throws -> Variant_NullType_BridgethingOtaProgress {
        Self.stateLock.lock()
        let backend = Self._backend
        Self.stateLock.unlock()
        let progress = backend.flatMap { $0.otaRunProgress(deviceId: deviceId, nowMs: nowMs) }
        return progress.map { .second($0) } ?? .first(NullType.null)
    }

    // MARK: - Webapp install

    public func installWebappFromUrl(
        deviceId: String,
        url: String,
        sha256: String,
        size: Double,
        provenance: Variant_NullType_String?,
        webappId: Variant_NullType_String?,
        webappName: Variant_NullType_String?
    ) throws -> Promise<BridgethingWebappInfo> {
        let prov = Self.unwrapString(provenance)
        let id = Self.unwrapString(webappId)
        let name = Self.unwrapString(webappName)
        return Promise.async {
            try await Self.backend().installWebappFromUrl(
                deviceId: deviceId, url: url, sha256: sha256, size: size, provenance: prov,
                webappId: id, webappName: name
            )
        }
    }

    // MARK: - Peer reconnect

    public func reconnectPeer(deviceId: String) throws -> Promise<Void> {
        Promise.async {
            try await Self.backend().reconnectPeer(deviceId: deviceId)
        }
    }

    // MARK: - Device nickname

    public func deviceSetNickname(deviceId: String, nickname: String) throws -> Promise<Void> {
        Promise.async {
            try await Self.backend().deviceSetNickname(deviceId: deviceId, nickname: nickname)
        }
    }

    // MARK: - Pair picker

    public func presentPairPicker() throws -> Promise<Variant_NullType_BridgethingBtDevice> {
        Promise.async {
            let device = try await Self.backend().presentPairPicker()
            return device.map { .second($0) } ?? .first(NullType.null)
        }
    }

    // MARK: - Notification access

    public func isNotificationAccessGranted() throws -> Promise<Bool> {
        Promise.async { await (try Self.backend()).isNotificationAccessGranted() }
    }

    public func requestNotificationAccess() throws -> Promise<Void> {
        Promise.async { try await Self.backend().requestNotificationAccess() }
    }

    public func isDefaultDialer() throws -> Promise<Bool> {
        Promise.async { await (try Self.backend()).isDefaultDialer() }
    }

    public func requestDefaultDialer() throws -> Promise<Void> {
        Promise.async { try await Self.backend().requestDefaultDialer() }
    }

    public func forgetCompanionDevice(mac: String) throws -> Promise<Void> {
        Promise.async { try await Self.backend().forgetCompanionDevice(mac: mac) }
    }

    public func isIgnoringBatteryOptimizations() throws -> Promise<Bool> {
        Promise.async { await (try Self.backend()).isIgnoringBatteryOptimizations() }
    }

    public func requestIgnoreBatteryOptimizations() throws -> Promise<Void> {
        Promise.async { try await Self.backend().requestIgnoreBatteryOptimizations() }
    }

    // MARK: - Runtime permission revoke

    public func revokeRuntimePermissions(permissions: [String]) throws -> Promise<Bool> {
        Promise.async { await (try Self.backend()).revokeRuntimePermissions(permissions: permissions) }
    }

    public func killApp() throws -> Promise<Void> {
        Promise.async { await (try Self.backend()).killApp() }
    }

    // MARK: - Callback setters

    public func setOnProvidersChanged(callback: @escaping ([BridgethingProviderInfo]) -> Void) throws {
        let wrapped: @Sendable ([BridgethingProviderInfo]) -> Void = { providers in callback(providers) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingProvidersChanged = wrapped }
        Self.stateLock.unlock()
        backend?.setOnProvidersChanged(wrapped)
    }

    public func setOnPeerConnected(callback: @escaping (BridgethingSessionPeer) -> Void) throws {
        let wrapped: @Sendable (BridgethingSessionPeer) -> Void = { peer in callback(peer) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingPeerConnected = wrapped }
        Self.stateLock.unlock()
        backend?.setOnPeerConnected(wrapped)
    }

    public func setOnPeerDisconnected(callback: @escaping (String) -> Void) throws {
        let wrapped: @Sendable (String) -> Void = { id in callback(id) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingPeerDisconnected = wrapped }
        Self.stateLock.unlock()
        backend?.setOnPeerDisconnected(wrapped)
    }

    public func setOnPeerLinkFailed(callback: @escaping (BridgethingSessionPeer) -> Void) throws {
        let wrapped: @Sendable (BridgethingSessionPeer) -> Void = { peer in callback(peer) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingPeerLinkFailed = wrapped }
        Self.stateLock.unlock()
        backend?.setOnPeerLinkFailed(wrapped)
    }

    public func setOnNowPlayingChanged(callback: @escaping (Variant_NullType_BridgethingNowPlaying?) -> Void) throws {
        let wrapped: @Sendable (BridgethingNowPlaying?) -> Void = { np in
            callback(np.map { .second($0) } ?? .first(NullType.null))
        }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingNowPlayingChanged = wrapped }
        Self.stateLock.unlock()
        backend?.setOnNowPlayingChanged(wrapped)
    }

    public func setOnAncsAuthStatusChanged(callback: @escaping (String, BridgethingAncsAuthStatus) -> Void) throws {
        let wrapped: @Sendable (String, BridgethingAncsAuthStatus) -> Void = { deviceId, status in
            callback(deviceId, status)
        }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingAncsAuthStatusChanged = wrapped }
        Self.stateLock.unlock()
        backend?.setOnAncsAuthStatusChanged(wrapped)
    }

    public func setOnLog(callback: @escaping (String, String, String) -> Void) throws {
        let wrapped: @Sendable (String, String, String) -> Void = { origin, level, msg in callback(origin, level, msg) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingLog = wrapped }
        Self.stateLock.unlock()
        backend?.setOnLog(wrapped)
    }

    public func setLogStreamingEnabled(enabled: Bool) throws {
        Self.stateLock.lock()
        let backend = Self._backend
        Self.stateLock.unlock()
        backend?.setLogStreamingEnabled(enabled)
    }

    public func setLocalLogStreamingEnabled(enabled: Bool) throws {
        Self.stateLock.lock()
        let backend = Self._backend
        Self.stateLock.unlock()
        backend?.setLocalLogStreamingEnabled(enabled)
    }

    public func setOnWebappsChanged(callback: @escaping (BridgethingDeviceWebappsEntry) -> Void) throws {
        let wrapped: @Sendable (BridgethingDeviceWebappsEntry) -> Void = { entry in callback(entry) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingWebappsChanged = wrapped }
        Self.stateLock.unlock()
        backend?.setOnWebappsChanged(wrapped)
    }

    public func setOnWebappDocChanged(callback: @escaping (String, String, String, Variant_NullType_String?) -> Void) throws {
        let wrapped: @Sendable (String, String, String, String?) -> Void = { deviceId, webappId, key, value in
            callback(deviceId, webappId, key, value.map { .second($0) } ?? .first(NullType.null))
        }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingWebappDocChanged = wrapped }
        Self.stateLock.unlock()
        backend?.setOnWebappDocChanged(wrapped)
    }

    public func setOnDeviceMetaChanged(callback: @escaping (String, BridgethingDeviceMeta) -> Void) throws {
        let wrapped: @Sendable (String, BridgethingDeviceMeta) -> Void = { id, meta in callback(id, meta) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingDeviceMetaChanged = wrapped }
        Self.stateLock.unlock()
        backend?.setOnDeviceMetaChanged(wrapped)
    }

    public func setOnVoiceModelStateChanged(callback: @escaping (BridgethingVoiceModelState) -> Void) throws {
        let wrapped: @Sendable (BridgethingVoiceModelState) -> Void = { state in callback(state) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingVoiceModelStateChanged = wrapped }
        Self.stateLock.unlock()
        backend?.setOnVoiceModelStateChanged(wrapped)
    }

    public func setOnVoiceTurnChanged(callback: @escaping (BridgethingVoiceTurn) -> Void) throws {
        let wrapped: @Sendable (BridgethingVoiceTurn) -> Void = { turn in callback(turn) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingVoiceTurnChanged = wrapped }
        Self.stateLock.unlock()
        backend?.setOnVoiceTurnChanged(wrapped)
    }

    public func setOnOtaRunChanged(callback: @escaping (BridgethingOtaRun) -> Void) throws {
        let wrapped: @Sendable (BridgethingOtaRun) -> Void = { run in callback(run) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingOtaRunChanged = wrapped }
        Self.stateLock.unlock()
        backend?.setOnOtaRunChanged(wrapped)
    }

    public func setOnOtaAvailableChanged(callback: @escaping (BridgethingOtaAvailable) -> Void) throws {
        let wrapped: @Sendable (BridgethingOtaAvailable) -> Void = { available in callback(available) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingOtaAvailableChanged = wrapped }
        Self.stateLock.unlock()
        backend?.setOnOtaAvailableChanged(wrapped)
    }

    public func setOnOtaPollChanged(callback: @escaping (BridgethingOtaPollStatus) -> Void) throws {
        let wrapped: @Sendable (BridgethingOtaPollStatus) -> Void = { status in callback(status) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingOtaPollChanged = wrapped }
        Self.stateLock.unlock()
        backend?.setOnOtaPollChanged(wrapped)
    }

    public func setOnResumed(callback: @escaping (BridgethingSessionSnapshot) -> Void) throws {
        let wrapped: @Sendable (BridgethingSessionSnapshot) -> Void = { snapshot in callback(snapshot) }
        Self.stateLock.lock()
        let backend = Self._backend
        if backend == nil { Self.pendingResumed = wrapped }
        Self.stateLock.unlock()
        backend?.setOnResumed(wrapped)
    }

}
