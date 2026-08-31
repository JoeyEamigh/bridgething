import BridgethingCompanion
import BridgethingCompanionCore
import BridgethingSession
import Foundation
import React
import UIKit

private final class ReloadDetacher: NSObject, RCTReloadListener {
    private let onReload: () -> Void
    init(onReload: @escaping () -> Void) {
        self.onReload = onReload
        super.init()
    }

    func didReceiveReloadCommand() {
        onReload()
    }
}

public final class HybridBridgethingSessionImpl: BridgethingSessionBackend, @unchecked Sendable {
    public static var hostInfo: HostInfo = .init(
        appName: "bridgething", appVersion: "0.0.0", osName: "iOS", osVersion: "", hostIdentifier: ""
    )

    public static var spotifyConfig: SpotifyProviderConfig?
    public static var eaProtocolString: String = "com.bridgething.gateway"

    private static let holderLock = NSLock()
    private static let devLaneDeviceId = "dev-gateway"
    private static var heldCompanion: BridgethingCompanion?
    private static var eventSink: (@Sendable (SessionEvent) -> Void)?

    private let stateLock = NSLock()
    private var companion: BridgethingCompanion?
    private var foreground = false
    private var foregroundGen: UInt64 = 0
    private var lifecycleObservers: [NSObjectProtocol] = []
    private var reloadDetacher: ReloadDetacher?
    private var logStreamingDesired = false
    private var localLogStreamingDesired = false

    private var onProvidersChanged: (@Sendable ([BridgethingProviderInfo]) -> Void)?
    private var onPeerConnected: (@Sendable (BridgethingSessionPeer) -> Void)?
    private var onPeerDisconnected: (@Sendable (String) -> Void)?
    private var onPeerLinkFailed: (@Sendable (BridgethingSessionPeer) -> Void)?
    private var onNowPlayingChanged: (@Sendable (BridgethingNowPlaying?) -> Void)?
    private var onAncsAuthStatusChanged: (@Sendable (String, BridgethingAncsAuthStatus) -> Void)?
    private var onLog: (@Sendable (String, String, String) -> Void)?
    private var onWebappsChanged: (@Sendable (BridgethingDeviceWebappsEntry) -> Void)?
    private var onWebappDocChanged: (@Sendable (String, String, String, String?) -> Void)?
    private var onDeviceMetaChanged: (@Sendable (String, BridgethingDeviceMeta) -> Void)?
    private var onVoiceModelStateChanged: (@Sendable (BridgethingVoiceModelState) -> Void)?
    private var onVoiceTurnChanged: (@Sendable (BridgethingVoiceTurn) -> Void)?
    private var onOtaRunChanged: (@Sendable (BridgethingOtaRun) -> Void)?
    private var onOtaAvailableChanged: (@Sendable (BridgethingOtaAvailable) -> Void)?
    private var onOtaPollChanged: (@Sendable (BridgethingOtaPollStatus) -> Void)?
    private var onResumed: (@Sendable (BridgethingSessionSnapshot) -> Void)?

    public init() {
        observeAppLifecycle()
        registerReloadDetach()
    }

    deinit {
        let center = NotificationCenter.default
        for token in lifecycleObservers { center.removeObserver(token) }
    }

    private func observeAppLifecycle() {
        let center = NotificationCenter.default
        let active = center.addObserver(
            forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in self?.resumeForeground() }
        let background = center.addObserver(
            forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main
        ) { [weak self] _ in
            self?.stateLock.withLock {
                self?.foreground = false
                self?.foregroundGen &+= 1
            }
        }
        lifecycleObservers = [active, background]
    }

    private func resumeForeground() {
        let companion = stateLock.withLock { () -> BridgethingCompanion? in
            foregroundGen &+= 1
            foreground = true
            return self.companion
        }
        guard let companion else { return }
        Task { await companion.resumed() }
    }

    private func registerReloadDetach() {
        let detacher = ReloadDetacher { [weak self] in self?.detachObservers() }
        reloadDetacher = detacher
        RCTRegisterReloadCommandListener(detacher)
    }

    private func detachObservers() {
        stateLock.withLock {
            onProvidersChanged = nil
            onPeerConnected = nil
            onPeerDisconnected = nil
            onPeerLinkFailed = nil
            onNowPlayingChanged = nil
            onAncsAuthStatusChanged = nil
            onLog = nil
            onWebappsChanged = nil
            onWebappDocChanged = nil
            onDeviceMetaChanged = nil
            onVoiceModelStateChanged = nil
            onVoiceTurnChanged = nil
            onOtaRunChanged = nil
            onOtaAvailableChanged = nil
            onOtaPollChanged = nil
            onResumed = nil
        }
    }

    // MARK: - Lifecycle

    private static func ensureStarted() async throws -> BridgethingCompanion {
        if let existing = holderLock.withLock({ heldCompanion }) { return existing }
        let companion = BridgethingCompanion(
            host: makeHostInfo(),
            capabilities: toCoreCapabilityFlags(loadCapabilityFlags()),
            spotify: spotifyConfig,
            eaProtocolString: eaProtocolString,
            events: { event in Self.holderLock.withLock { Self.eventSink }?(event) }
        )
        let raced: BridgethingCompanion? = holderLock.withLock {
            if let existing = heldCompanion { return existing }
            heldCompanion = companion
            return nil
        }
        if let raced { return raced }
        try await companion.start()
        return companion
    }

    public func start() async throws {
        Self.holderLock.withLock {
            Self.eventSink = { [weak self] event in self?.handleSessionEvent(event) }
        }
        let companion = try await Self.ensureStarted()
        let firstAttach = stateLock.withLock { () -> Bool in
            if self.companion != nil { return false }
            self.companion = companion
            return true
        }
        guard firstAttach else { return }

        let (localDesired, deviceDesired) = stateLock.withLock {
            (localLogStreamingDesired, logStreamingDesired)
        }
        if localDesired { CompanionLogRelay.shared.setInbox(companion.logInbox()) }
        if deviceDesired { await companion.session.setDeviceLogStreaming(enabled: true) }
        await replayHostSettings(companion.session)
        connectDevGateway(companion.session)
    }

    private func connectDevGateway(_ session: CompanionSession) {
        guard let url = Bundle.main.object(forInfoDictionaryKey: "BRIDGETHING_DEV_GATEWAY") as? String, !url.isEmpty else {
            return
        }
        Task {
            do {
                try await session.connectNetwork(url: url, device: LinkDevice(id: Self.devLaneDeviceId, name: "dev gateway"))
            } catch {
                NSLog("bridgething.session dev gateway %@: %@", url, String(describing: error))
            }
        }
    }

    public func stop() async {
        stateLock.withLock { self.companion = nil }
        Self.holderLock.withLock { Self.eventSink = nil }
        CompanionLogRelay.shared.setInbox(nil)
        emitNowPlaying(nil)
    }

    private func handleSessionEvent(_ event: SessionEvent) {
        let foreground = stateLock.withLock { self.foreground }
        switch event {
        case let .providersChanged(providers):
            if foreground {
                stateLock.withLock { onProvidersChanged }?(providers.map(toRNProviderInfo))
            }
        case let .peerConnected(peer):
            if foreground { stateLock.withLock { onPeerConnected }?(toRNPeer(peer)) }
        case let .peerDisconnected(deviceId):
            if foreground { stateLock.withLock { onPeerDisconnected }?(deviceId) }
        case let .peerLinkFailed(peer):
            if foreground { stateLock.withLock { onPeerLinkFailed }?(toRNPeer(peer)) }
        case let .nowPlayingChanged(nowPlaying):
            emitNowPlaying(nowPlaying.map(toRNNowPlaying))
        case let .ancsAuthStatusChanged(deviceId, status):
            if foreground {
                stateLock.withLock { onAncsAuthStatusChanged }?(deviceId, toRNAncsAuthStatus(status))
            }
        case let .log(origin, level, target, message):
            let line = "[\(target)] \(message)"
            if origin == .device {
                CompanionLogs.shared.store?.record(level: toStoreLevel(level), label: daemonLabel, message: line)
            }
            if foreground { stateLock.withLock { onLog }?(toOriginName(origin), toLevelName(level), line) }
        case let .webappsChanged(entry):
            if foreground { stateLock.withLock { onWebappsChanged }?(toRNWebappsEntry(entry)) }
        case let .webappDocChanged(deviceId, webappId, key, value):
            if foreground {
                stateLock.withLock { onWebappDocChanged }?(deviceId, webappId.lowercased(), key, value)
            }
        case let .deviceMetaChanged(deviceId, meta):
            if foreground {
                stateLock.withLock { onDeviceMetaChanged }?(deviceId, toRNDeviceMeta(meta))
            }
        case let .voiceModelStateChanged(state):
            if foreground {
                stateLock.withLock { onVoiceModelStateChanged }?(toRNVoiceModelState(state))
            }
        case let .voiceTurnChanged(turn):
            if foreground {
                stateLock.withLock { onVoiceTurnChanged }?(toRNVoiceTurn(turn))
            }
        case let .otaRunChanged(run):
            if foreground { stateLock.withLock { onOtaRunChanged }?(toRNOtaRun(run)) }
        case let .otaAvailableChanged(available):
            if foreground { stateLock.withLock { onOtaAvailableChanged }?(toRNOtaAvailable(available)) }
        case let .otaPollChanged(status):
            if foreground { stateLock.withLock { onOtaPollChanged }?(toRNOtaPollStatus(status)) }
        case .companionUpdateProgress:
            break
        case .resumed:
            let gen = stateLock.withLock { foregroundGen }
            Task { [weak self] in
                guard let self else { return }
                let snapshot = await self.snapshot()
                let callback = self.stateLock.withLock {
                    () -> (@Sendable (BridgethingSessionSnapshot) -> Void)? in
                    guard self.foregroundGen == gen, self.foreground else { return nil }
                    return self.onResumed
                }
                callback?(snapshot)
            }
        }
    }

    private func emitNowPlaying(_ np: BridgethingNowPlaying?) {
        stateLock.withLock { foreground ? onNowPlayingChanged : nil }?(np)
    }

    private func requireSession() throws -> CompanionSession {
        guard let companion = stateLock.withLock({ self.companion }) else {
            throw SessionError.notStarted
        }
        return companion.session
    }

    private func requireCompanion() throws -> BridgethingCompanion {
        guard let companion = stateLock.withLock({ self.companion }) else {
            throw SessionError.notStarted
        }
        return companion
    }

    // MARK: - Providers

    public func availableProviders() async -> [BridgethingProviderInfo] {
        guard let session = try? requireSession() else { return [] }
        return await session.availableProviders().map(toRNProviderInfo)
    }

    public func connectProvider(id: String) async throws {
        try await requireSession().connectProvider(id: id)
    }

    public func cancelAuth(id: String) async {
        try? await requireSession().cancelAuth(id: id)
    }

    public func disconnectProvider(id: String) async {
        try? await requireSession().disconnectProvider(id: id)
    }

    public func setProviderPriority(ids: [String]) async {
        Self.defaults.set(ids, forKey: PrefKey.providerPriority)
        guard let session = try? requireSession() else { return }
        await session.setProviderPriority(ids: ids)
    }

    // MARK: - Snapshot + logs

    public func snapshot() async -> BridgethingSessionSnapshot {
        guard let session = try? requireSession() else { return emptySnapshot() }
        return toRNSnapshot(await session.snapshot())
    }

    public func deviceLogSnapshot(limit: Double) async -> [BridgethingDeviceLogLine] {
        guard let session = try? requireSession() else { return [] }
        return session.deviceLogSnapshot(limit: UInt32(clamping: Int(max(0, limit)))).map {
            BridgethingDeviceLogLine(
                seq: Double($0.seq),
                ts: Double($0.tsUnixMs),
                origin: toOriginName($0.origin),
                level: toLevelName($0.level),
                message: "[\($0.target)] \($0.message)"
            )
        }
    }

    public func companionDebug() async throws -> BridgethingCompanionDebug {
        toRNCompanionDebug(try requireSession().companionDebug())
    }

    public func persistedLogSize() async -> Double {
        await onDisk { Double(CompanionLogs.shared.store?.retainedBytes() ?? 0) }
    }

    public func logArchives() async -> [BridgethingLogArchive] {
        await onDisk {
            (CompanionLogs.shared.store?.archives() ?? []).map {
                BridgethingLogArchive(
                    id: $0.id,
                    startedAt: Double($0.startedAtMs),
                    bytes: Double($0.bytes),
                    pinned: $0.pinned,
                    current: $0.current
                )
            }
        }
    }

    public func logArchiveLines(archiveId: String, limit: Double) async -> [BridgethingDeviceLogLine] {
        await onDisk {
            let lines = CompanionLogs.shared.store?.read(
                id: archiveId,
                limit: UInt32(clamping: Int(max(0, limit)))
            ) ?? []
            return lines.enumerated().map { index, line in
                BridgethingDeviceLogLine(
                    seq: Double(index),
                    ts: Double(line.tsUnixMs),
                    origin: line.label == daemonLabel ? "device" : "local",
                    level: toLevelName(line.level),
                    message: line.label.isEmpty ? line.message : "[\(line.label)] \(line.message)"
                )
            }
        }
    }

    public func exportLogs(archiveId: String?) async throws -> String {
        try await Task.detached(priority: .utility) {
            try LogExport.writeBundle(archiveId: archiveId).path
        }.value
    }

    public func shareLogs(archiveId: String?) async -> Bool {
        let file = await Task.detached(priority: .utility) {
            try? LogExport.writeBundle(archiveId: archiveId)
        }.value
        guard let file else { return false }
        return await MainActor.run { LogExport.share(file) }
    }

    public func deleteLogArchive(archiveId: String) async {
        await onDisk { CompanionLogs.shared.store?.delete(id: archiveId) ?? () }
    }

    public func clearPersistedLogs() async {
        await onDisk { CompanionLogs.shared.store?.clear() ?? () }
    }

    private func onDisk<T: Sendable>(_ work: @escaping @Sendable () -> T) async -> T {
        await Task.detached(priority: .utility, operation: work).value
    }

    public func setLogStreamingEnabled(_ enabled: Bool) {
        let companion = stateLock.withLock { () -> BridgethingCompanion? in
            logStreamingDesired = enabled
            return self.companion
        }
        guard let companion else { return }
        Task { await companion.session.setDeviceLogStreaming(enabled: enabled) }
    }

    public func setLocalLogStreamingEnabled(_ enabled: Bool) {
        let companion = stateLock.withLock { () -> BridgethingCompanion? in
            localLogStreamingDesired = enabled
            return self.companion
        }
        guard let companion else { return }
        CompanionLogRelay.shared.setInbox(enabled ? companion.logInbox() : nil)
    }

    // MARK: - ANCS

    public func enableAncsNotifications(deviceId: String) async -> BridgethingAncsSetupResult {
        guard let companion = stateLock.withLock({ self.companion }) else {
            return BridgethingAncsSetupResult(
                kind: .failed, authStatus: .unknown, message: "session not started"
            )
        }
        return toRNAncsSetupResult(await companion.enableAncsNotifications(deviceId: deviceId))
    }

    public func ancsAuthStatus(deviceId: String) async -> BridgethingAncsAuthStatus {
        guard let companion = stateLock.withLock({ self.companion }) else { return .unknown }
        return toRNAncsAuthStatus(companion.currentAncsAuthState(deviceId: deviceId))
    }

    // MARK: - Webapps (per-device)

    public func listWebapps(deviceId: String) async throws -> [BridgethingWebappInfo] {
        try await requireSession().listWebapps(deviceId: deviceId)
            .filter { $0.role != .launcher }
            .map(toRNWebappInfo)
    }

    public func currentWebapp(deviceId: String) async throws -> BridgethingActiveWebapp? {
        try await requireSession().currentWebapp(deviceId: deviceId).map(toRNActiveWebapp)
    }

    public func installWebapp(deviceId: String, sourceUri: String) async throws -> BridgethingWebappInfo {
        let session = try requireSession()
        guard let url = URL(string: sourceUri) else { throw SessionError.invalidArchive }
        let info: WebappInfo
        if url.isFileURL {
            info = try await session.installWebapp(deviceId: deviceId, archivePath: url.path, provenance: nil)
        } else if let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https" {
            info = try await session.installWebappFromUrl(
                deviceId: deviceId, url: sourceUri, expected: nil, provenance: sourceUri
            )
        } else {
            throw SessionError.invalidArchive
        }
        return toRNWebappInfo(info)
    }

    public func installWebappFromUrl(
        deviceId: String,
        url: String,
        sha256: String,
        size: Double,
        provenance: String?,
        webappId: String?,
        webappName: String?
    ) async throws -> BridgethingWebappInfo {
        _ = (webappId, webappName)
        let info = try await requireSession().installWebappFromUrl(
            deviceId: deviceId,
            url: url,
            expected: ArtifactDigest(size: UInt64(max(0, size)), sha256: sha256.lowercased()),
            provenance: provenance
        )
        return toRNWebappInfo(info)
    }

    public func uninstallWebapp(deviceId: String, id: String) async throws {
        try await requireSession().uninstallWebapp(deviceId: deviceId, id: id)
    }

    public func switchWebapp(deviceId: String, id: String) async throws {
        try await requireSession().switchWebapp(deviceId: deviceId, id: id)
    }

    public func getWebappSlots(deviceId: String) async throws -> BridgethingWebappSlots {
        toRNWebappSlots(try await requireSession().webappSlots(deviceId: deviceId))
    }

    public func setWebappSlot(deviceId: String, slot: BridgethingWebappSlot, id: String?) async throws
        -> BridgethingWebappSlots
    {
        toRNWebappSlots(
            try await requireSession().setWebappSlot(
                deviceId: deviceId, slot: toCoreWebappSlot(slot), id: id
            )
        )
    }

    public func webappIcon(deviceId: String, id: String) async throws -> BridgethingWebappIcon? {
        let resolved: WebappResourceFile
        do {
            resolved = try await requireSession().webappResource(deviceId: deviceId, id: id, kind: .icon, origin: nil)
        } catch CompanionError.ResourceNotAvailable {
            return nil
        }
        let file = URL(fileURLWithPath: resolved.path)
        if resolved.mime == "image/svg+xml", let svg = try? String(contentsOf: file, encoding: .utf8) {
            return BridgethingWebappIcon(fileUri: nil, svg: svg, mime: resolved.mime)
        }
        return BridgethingWebappIcon(fileUri: file.absoluteString, svg: nil, mime: resolved.mime)
    }

    public func webappSettingsMarkup(deviceId: String, id: String, origin: BridgethingResourceOrigin?) async throws -> String {
        let resolved = try await requireSession().webappResource(
            deviceId: deviceId,
            id: id,
            kind: .settings,
            origin: origin.map {
                WebappResourceOrigin(url: $0.url, sha256: $0.sha256, size: UInt64($0.size), mime: $0.mime)
            }
        )
        return try String(contentsOf: URL(fileURLWithPath: resolved.path), encoding: .utf8)
    }

    public func listWebappConfig(deviceId: String, id: String) async throws -> [BridgethingConfigEntry] {
        try await requireSession().listWebappConfig(deviceId: deviceId, id: id).map {
            BridgethingConfigEntry(key: $0.key, value: $0.value)
        }
    }

    public func setWebappConfigField(deviceId: String, id: String, key: String, value: String) async throws {
        try await requireSession().setWebappConfigField(deviceId: deviceId, id: id, key: key, value: value)
    }

    public func deleteWebappConfigField(deviceId: String, id: String, key: String) async throws {
        try await requireSession().deleteWebappConfigField(deviceId: deviceId, id: id, key: key)
    }

    public func getWebappDoc(deviceId: String, id: String, key: String) async throws -> String? {
        try await requireSession().getWebappDoc(deviceId: deviceId, id: id, key: key)
    }

    public func listWebappDoc(deviceId: String, id: String) async throws -> [BridgethingDocEntry] {
        try await requireSession().listWebappDoc(deviceId: deviceId, id: id).map {
            BridgethingDocEntry(key: $0.key, value: $0.value)
        }
    }

    public func setWebappDoc(deviceId: String, id: String, key: String, value: String) async throws {
        try await requireSession().setWebappDoc(deviceId: deviceId, id: id, key: key, value: value)
    }

    public func deleteWebappDoc(deviceId: String, id: String, key: String) async throws {
        try await requireSession().deleteWebappDoc(deviceId: deviceId, id: id, key: key)
    }

    // MARK: - Capability flags + voice model

    public func setCapabilityFlags(flags: BridgethingCapabilityFlags) async {
        Self.saveCapabilityFlags(flags)
        guard let session = try? requireSession() else { return }
        await session.setCapabilityFlags(flags: toCoreCapabilityFlags(flags))
    }

    public func voiceModelState() async -> BridgethingVoiceModelState {
        guard let session = try? requireSession() else {
            return BridgethingVoiceModelState(
                status: .absent, receivedBytes: 0, totalBytes: 0, version: nil, error: nil
            )
        }
        return toRNVoiceModelState(await session.snapshot().voiceModel)
    }

    public func downloadVoiceModel() async {
        guard let session = try? requireSession() else { return }
        await session.downloadVoiceModel()
    }

    // MARK: - OTA

    public func setDeviceAutoResume(deviceId: String, enabled: Bool) async {
        var map = Self.loadAutoResumeMap()
        map[deviceId] = enabled
        Self.defaults.set(map, forKey: PrefKey.autoResume)
        guard let session = try? requireSession() else { return }
        await session.setDeviceAutoResume(deviceId: deviceId, enabled: enabled)
    }

    public func isDeviceAutoResumeEnabled(deviceId: String) async -> Bool {
        Self.loadAutoResumeMap()[deviceId] ?? true
    }

    private static func loadAutoResumeMap() -> [String: Bool] {
        defaults.dictionary(forKey: PrefKey.autoResume) as? [String: Bool] ?? [:]
    }

    public func setDeviceResumeTarget(deviceId: String, target: BridgethingResumeTarget) async {
        var map = Self.loadResumeTargetMap()
        map[deviceId] = target.stringValue
        Self.defaults.set(map, forKey: PrefKey.resumeTarget)
        guard let session = try? requireSession() else { return }
        await session.setDeviceResumeTarget(deviceId: deviceId, target: toCoreResumeTarget(target))
    }

    public func deviceResumeTarget(deviceId: String) async -> BridgethingResumeTarget {
        Self.loadResumeTargetMap()[deviceId].flatMap { BridgethingResumeTarget(fromString: $0) } ?? .phoneonly
    }

    private static func loadResumeTargetMap() -> [String: String] {
        defaults.dictionary(forKey: PrefKey.resumeTarget) as? [String: String] ?? [:]
    }

    public func setOtaPollConfig(config: BridgethingOtaPollConfig?) async {
        Self.saveOtaPollConfig(config)
        guard let session = try? requireSession() else { return }
        await session.setOtaPollConfig(config: config.map(toCoreOtaPollConfig))
    }

    public func checkForOtaUpdate(rootUrl: String) async {
        guard let session = try? requireSession() else { return }
        await session.checkForOtaUpdate(rootUrl: rootUrl)
    }

    public func fetchOtaManifest(rootUrl: String) async throws -> BridgethingOtaManifest {
        toRNOtaManifest(try await requireSession().fetchOtaManifest(rootUrl: rootUrl))
    }

    public func applyOtaUpdate(deviceId: String, channel: String, version: String, rootUrl: String) async throws {
        try await requireSession().applyOtaUpdate(
            deviceId: deviceId, channel: channel, version: version, rootUrl: rootUrl
        )
    }

    public func otaRunProgress(deviceId: String, nowMs: Double) -> BridgethingOtaProgress? {
        guard let session = try? requireSession() else { return nil }
        return session.otaRunProgress(deviceId: deviceId, nowMs: UInt64(max(0, nowMs))).map(toRNOtaProgress)
    }

    public func dismissOtaRun(deviceId: String) async throws {
        try await requireSession().dismissOtaRun(deviceId: deviceId)
    }

    // MARK: - Peers

    public func reconnectPeer(deviceId: String) async throws {
        try requireCompanion().transport.reconnect(deviceId: deviceId)
    }

    public func deviceSetNickname(deviceId: String, nickname: String) async throws {
        try await requireSession().deviceSetNickname(deviceId: deviceId, nickname: nickname)
    }

    public func presentPairPicker() async throws -> BridgethingBtDevice? {
        guard let result = await requireCompanionOrNil()?.presentPairPicker() else { return nil }
        return BridgethingBtDevice(
            address: result.id,
            name: result.name,
            bondState: .bonded,
            isCarThing: true
        )
    }

    private func requireCompanionOrNil() -> BridgethingCompanion? {
        stateLock.withLock { self.companion }
    }

    // MARK: - Android-only surfaces

    public func isNotificationAccessGranted() async -> Bool { false }
    public func requestNotificationAccess() async throws { throw SessionError.unsupportedOnPlatform }
    public func isDefaultDialer() async -> Bool { false }
    public func requestDefaultDialer() async throws { throw SessionError.unsupportedOnPlatform }
    public func installCompanionUpdate(url: String, filename: String, size: Double, sha256: String) async throws {
        throw SessionError.unsupportedOnPlatform
    }
    public func forgetCompanionDevice(mac: String) async throws {}
    public func isIgnoringBatteryOptimizations() async -> Bool { false }
    public func requestIgnoreBatteryOptimizations() async throws { throw SessionError.unsupportedOnPlatform }
    public func revokeRuntimePermissions(permissions: [String]) async -> Bool { false }
    public func killApp() async {
        // apple rejects explicit process termination.
    }

    // MARK: - Callback setters

    public func setOnProvidersChanged(_ callback: @escaping @Sendable ([BridgethingProviderInfo]) -> Void) {
        stateLock.withLock { onProvidersChanged = callback }
    }

    public func setOnPeerConnected(_ callback: @escaping @Sendable (BridgethingSessionPeer) -> Void) {
        stateLock.withLock { onPeerConnected = callback }
    }

    public func setOnPeerDisconnected(_ callback: @escaping @Sendable (String) -> Void) {
        stateLock.withLock { onPeerDisconnected = callback }
    }

    public func setOnPeerLinkFailed(_ callback: @escaping @Sendable (BridgethingSessionPeer) -> Void) {
        stateLock.withLock { onPeerLinkFailed = callback }
    }

    public func setOnNowPlayingChanged(_ callback: @escaping @Sendable (BridgethingNowPlaying?) -> Void) {
        stateLock.withLock { onNowPlayingChanged = callback }
    }

    public func setOnAncsAuthStatusChanged(_ callback: @escaping @Sendable (String, BridgethingAncsAuthStatus) -> Void) {
        stateLock.withLock { onAncsAuthStatusChanged = callback }
    }

    public func setOnLog(_ callback: @escaping @Sendable (String, String, String) -> Void) {
        stateLock.withLock { onLog = callback }
    }

    public func setOnWebappsChanged(_ callback: @escaping @Sendable (BridgethingDeviceWebappsEntry) -> Void) {
        stateLock.withLock { onWebappsChanged = callback }
    }

    public func setOnWebappDocChanged(_ callback: @escaping @Sendable (String, String, String, String?) -> Void) {
        stateLock.withLock { onWebappDocChanged = callback }
    }

    public func setOnDeviceMetaChanged(_ callback: @escaping @Sendable (String, BridgethingDeviceMeta) -> Void) {
        stateLock.withLock { onDeviceMetaChanged = callback }
    }

    public func setOnVoiceModelStateChanged(_ callback: @escaping @Sendable (BridgethingVoiceModelState) -> Void) {
        stateLock.withLock { onVoiceModelStateChanged = callback }
    }

    public func setOnVoiceTurnChanged(_ callback: @escaping @Sendable (BridgethingVoiceTurn) -> Void) {
        stateLock.withLock { onVoiceTurnChanged = callback }
    }

    public func setOnOtaRunChanged(_ callback: @escaping @Sendable (BridgethingOtaRun) -> Void) {
        stateLock.withLock { onOtaRunChanged = callback }
    }

    public func setOnOtaAvailableChanged(_ callback: @escaping @Sendable (BridgethingOtaAvailable) -> Void) {
        stateLock.withLock { onOtaAvailableChanged = callback }
    }

    public func setOnOtaPollChanged(_ callback: @escaping @Sendable (BridgethingOtaPollStatus) -> Void) {
        stateLock.withLock { onOtaPollChanged = callback }
    }

    public func setOnCompanionUpdateProgress(_ callback: @escaping @Sendable (Double, Double) -> Void) {}

    public func setOnResumed(_ callback: @escaping @Sendable (BridgethingSessionSnapshot) -> Void) {
        stateLock.withLock { onResumed = callback }
    }

    // MARK: - Host identity + persisted settings

    private static func makeHostInfo() -> HostInfo {
        let base = hostInfo
        return HostInfo(
            appName: base.appName,
            appVersion: base.appVersion,
            osName: base.osName,
            osVersion: UIDevice.current.systemVersion,
            hostIdentifier: UIDevice.current.identifierForVendor?.uuidString ?? ""
        )
    }

    private func replayHostSettings(_ session: CompanionSession) async {
        await session.setCapabilityFlags(flags: toCoreCapabilityFlags(Self.loadCapabilityFlags()))
        await session.setOtaPollConfig(config: Self.loadOtaPollConfig().map(toCoreOtaPollConfig))
        for (deviceId, enabled) in Self.loadAutoResumeMap() {
            await session.setDeviceAutoResume(deviceId: deviceId, enabled: enabled)
        }
        for (deviceId, raw) in Self.loadResumeTargetMap() {
            guard let target = BridgethingResumeTarget(fromString: raw) else { continue }
            await session.setDeviceResumeTarget(deviceId: deviceId, target: toCoreResumeTarget(target))
        }
        let priority = Self.defaults.stringArray(forKey: PrefKey.providerPriority) ?? []
        await session.setProviderPriority(ids: priority)
    }

    private static let defaults = UserDefaults.standard

    private enum PrefKey {
        static let capsConfigured = "bridgething.caps.configured"
        static let capsGeo = "bridgething.caps.geo"
        static let capsNotifications = "bridgething.caps.notifications"
        static let capsNetFetch = "bridgething.caps.netFetch"
        static let capsNetWs = "bridgething.caps.netWs"
        static let capsAudioTts = "bridgething.caps.audioTts"
        static let capsVoiceModel = "bridgething.caps.voiceModel"
        static let autoResume = "bridgething.autoresume"
        static let resumeTarget = "bridgething.resumeTarget"
        static let otaConfigured = "bridgething.ota.configured"
        static let otaInterval = "bridgething.ota.intervalSeconds"
        static let otaAutoPush = "bridgething.ota.autoPush"
        static let otaRootUrl = "bridgething.ota.rootUrl"
        static let providerPriority = "bridgething.providerPriority"
    }

    private static func loadCapabilityFlags() -> BridgethingCapabilityFlags {
        guard defaults.bool(forKey: PrefKey.capsConfigured) else {
            return BridgethingCapabilityFlags(
                geo: true, notifications: true, netFetch: true, netWs: true, audioTts: true,
                voiceModel: true
            )
        }
        return BridgethingCapabilityFlags(
            geo: defaults.bool(forKey: PrefKey.capsGeo),
            notifications: defaults.bool(forKey: PrefKey.capsNotifications),
            netFetch: defaults.bool(forKey: PrefKey.capsNetFetch),
            netWs: defaults.bool(forKey: PrefKey.capsNetWs),
            audioTts: defaults.bool(forKey: PrefKey.capsAudioTts),
            voiceModel: defaults.bool(forKey: PrefKey.capsVoiceModel)
        )
    }

    private static func saveCapabilityFlags(_ f: BridgethingCapabilityFlags) {
        defaults.set(true, forKey: PrefKey.capsConfigured)
        defaults.set(f.geo, forKey: PrefKey.capsGeo)
        defaults.set(f.notifications, forKey: PrefKey.capsNotifications)
        defaults.set(f.netFetch, forKey: PrefKey.capsNetFetch)
        defaults.set(f.netWs, forKey: PrefKey.capsNetWs)
        defaults.set(f.audioTts, forKey: PrefKey.capsAudioTts)
        defaults.set(f.voiceModel, forKey: PrefKey.capsVoiceModel)
    }

    private static func loadOtaPollConfig() -> BridgethingOtaPollConfig? {
        guard defaults.bool(forKey: PrefKey.otaConfigured) else {
            return BridgethingOtaPollConfig(intervalSeconds: 3600, autoPush: true, rootUrl: nil)
        }
        let root = defaults.string(forKey: PrefKey.otaRootUrl)
        return BridgethingOtaPollConfig(
            intervalSeconds: defaults.double(forKey: PrefKey.otaInterval),
            autoPush: defaults.object(forKey: PrefKey.otaAutoPush) == nil
                ? true : defaults.bool(forKey: PrefKey.otaAutoPush),
            rootUrl: (root?.isEmpty == false) ? root : nil
        )
    }

    private static func saveOtaPollConfig(_ config: BridgethingOtaPollConfig?) {
        guard let config else {
            defaults.set(false, forKey: PrefKey.otaConfigured)
            return
        }
        defaults.set(true, forKey: PrefKey.otaConfigured)
        defaults.set(config.intervalSeconds, forKey: PrefKey.otaInterval)
        defaults.set(config.autoPush, forKey: PrefKey.otaAutoPush)
        defaults.set(config.rootUrl, forKey: PrefKey.otaRootUrl)
    }

    private func emptySnapshot() -> BridgethingSessionSnapshot {
        BridgethingSessionSnapshot(
            hostInfo: BridgethingHostInfo(
                appName: Self.hostInfo.appName,
                appVersion: Self.hostInfo.appVersion,
                osName: Self.hostInfo.osName,
                osVersion: UIDevice.current.systemVersion,
                hostIdentifier: "",
                libVersion: "",
                libbridgethingVersion: "",
                adapterVersion: "eaccessory"
            ),
            providers: [],
            providerPriority: [],
            libraryProvider: nil,
            peers: [],
            ancsAuthStatuses: [],
            nowPlaying: nil,
            deviceMeta: [],
            capabilityFlags: Self.loadCapabilityFlags(),
            voiceModel: BridgethingVoiceModelState(
                status: .absent, receivedBytes: 0, totalBytes: 0, version: nil, error: nil
            ),
            otaPollConfig: Self.loadOtaPollConfig(),
            webapps: [],
            otaRuns: [],
            otaAvailable: [],
            otaPoll: BridgethingOtaPollStatus(lastPolledAt: nil, error: nil)
        )
    }
}

private enum SessionError: Error {
    case notStarted
    case invalidArchive
    case unsupportedOnPlatform
}

private extension NSLock {
    @discardableResult
    func withLock<T>(_ body: () throws -> T) rethrows -> T {
        lock(); defer { unlock() }
        return try body()
    }
}

// MARK: - Core -> RN projections

private func visible(_ webapps: [WebappInfo]) -> [WebappInfo] {
    webapps.filter { $0.role != .launcher }
}

private func toRNSnapshot(_ snap: SessionSnapshot) -> BridgethingSessionSnapshot {
    BridgethingSessionSnapshot(
        hostInfo: BridgethingHostInfo(
            appName: snap.hostInfo.appName,
            appVersion: snap.hostInfo.appVersion,
            osName: snap.hostInfo.osName,
            osVersion: snap.hostInfo.osVersion,
            hostIdentifier: snap.hostInfo.hostIdentifier,
            libVersion: snap.hostInfo.libVersion,
            libbridgethingVersion: snap.hostInfo.libbridgethingVersion,
            adapterVersion: "eaccessory"
        ),
        providers: snap.providers.map(toRNProviderInfo),
        providerPriority: snap.providerPriority,
        libraryProvider: snap.libraryProvider,
        peers: snap.peers.map(toRNPeer),
        ancsAuthStatuses: snap.ancsAuthStatuses.map {
            BridgethingAncsAuthStatusEntry(deviceId: $0.deviceId, status: toRNAncsAuthStatus($0.status))
        },
        nowPlaying: snap.nowPlaying.map(toRNNowPlaying),
        deviceMeta: snap.deviceMeta.map {
            BridgethingDeviceMetaEntry(deviceId: $0.deviceId, meta: toRNDeviceMeta($0.meta))
        },
        capabilityFlags: toRNCapabilityFlags(snap.capabilityFlags),
        voiceModel: toRNVoiceModelState(snap.voiceModel),
        otaPollConfig: snap.otaPollConfig.map(toRNOtaPollConfig),
        webapps: snap.webapps.map(toRNWebappsEntry),
        otaRuns: snap.otaRuns.map(toRNOtaRun),
        otaAvailable: snap.otaAvailable.map(toRNOtaAvailable),
        otaPoll: toRNOtaPollStatus(snap.otaPoll)
    )
}

private func toRNProviderInfo(_ info: ProviderInfo) -> BridgethingProviderInfo {
    let healthKind: BridgethingServiceHealthKind = switch info.serviceHealth.kind {
    case .ok: .ok
    case .rateLimited: .ratelimited
    case .unreachable: .unreachable
    }
    return BridgethingProviderInfo(
        id: info.id,
        displayName: info.displayName,
        available: info.available,
        connected: info.connected,
        authState: toRNAuthState(info.authState),
        serviceHealth: BridgethingServiceHealth(
            kind: healthKind,
            retryAfterSeconds: info.serviceHealth.retryAfterSeconds.map(Double.init)
        )
    )
}

private func toRNAuthState(_ state: AuthState) -> BridgethingAuthState {
    let kind: BridgethingAuthKind = switch state.kind {
    case .idle: .idle
    case .pending: .pending
    case .authenticated: .authenticated
    case .failed: .failed
    }
    return BridgethingAuthState(
        kind: kind,
        userCode: state.userCode,
        verificationUrl: state.verificationUrl,
        verificationUrlComplete: state.verificationUrlComplete,
        message: state.message
    )
}

private func toRNPeer(_ peer: SessionPeer) -> BridgethingSessionPeer {
    BridgethingSessionPeer(
        id: peer.id,
        name: peer.name,
        status: peer.status == .connected ? .connected : .linkfailed,
        linkError: peer.linkError
    )
}

private func toRNNowPlaying(_ np: NowPlaying) -> BridgethingNowPlaying {
    let track: BridgethingNowPlayingTrack? = np.track.map { t in
        BridgethingNowPlayingTrack(
            id: t.id,
            title: t.title,
            artist: t.artist,
            album: t.album,
            artworkUrl: t.artworkUrl,
            durationMs: t.durationMs.map(Double.init)
        )
    }
    let mode: BridgethingRepeatMode = switch np.playback.repeatMode {
    case .off: .off
    case .one: .one
    case .all: .all
    }
    return BridgethingNowPlaying(
        track: track,
        playback: BridgethingNowPlayingPlayback(
            playing: np.playback.playing,
            positionMs: Double(np.playback.positionMs),
            shuffle: np.playback.shuffle,
            repeatMode: mode
        ),
        appName: np.appName
    )
}

private func toRNAncsAuthStatus(_ status: AncsAuthStatus) -> BridgethingAncsAuthStatus {
    switch status {
    case .unknown: .unknown
    case .probing: .probing
    case .authorized: .authorized
    case .unauthorized: .unauthorized
    }
}

private func toRNAncsSetupResult(_ result: AncsSetupResult) -> BridgethingAncsSetupResult {
    let (kind, message): (BridgethingAncsSetupKind, String?) = switch result.kind {
    case .paired: (.paired, nil)
    case .alreadyPaired: (.alreadypaired, nil)
    case .cancelled: (.cancelled, nil)
    case .unsupported: (.unsupported, nil)
    case let .failed(reason): (.failed, reason)
    }
    return BridgethingAncsSetupResult(
        kind: kind,
        authStatus: toRNAncsAuthStatus(result.authState),
        message: message
    )
}

private func toRNDeviceMeta(_ meta: DeviceMeta) -> BridgethingDeviceMeta {
    BridgethingDeviceMeta(
        daemonVersion: meta.daemonVersion,
        libbridgethingVersion: meta.libbridgethingVersion,
        imageVersion: meta.imageVersion,
        appName: meta.appName,
        osName: meta.osName,
        osVersion: meta.osVersion,
        channel: meta.channel,
        modelName: meta.modelName,
        serialNumber: meta.serialNumber,
        nickname: meta.nickname
    )
}

private func toRNCapabilityFlags(_ flags: CapabilityFlags) -> BridgethingCapabilityFlags {
    BridgethingCapabilityFlags(
        geo: flags.geo,
        notifications: flags.notifications,
        netFetch: flags.netFetch,
        netWs: flags.netWs,
        audioTts: flags.audioTts,
        voiceModel: flags.voiceModel
    )
}

private func toCoreCapabilityFlags(_ flags: BridgethingCapabilityFlags) -> CapabilityFlags {
    CapabilityFlags(
        geo: flags.geo,
        notifications: flags.notifications,
        netFetch: flags.netFetch,
        netWs: flags.netWs,
        audioTts: flags.audioTts,
        voiceModel: flags.voiceModel
    )
}

private func toRNCompanionDebug(_ debug: CompanionDebug) -> BridgethingCompanionDebug {
    BridgethingCompanionDebug(
        authorityPlaybackHeld: debug.authorityPlaybackHeld,
        authorityMetadataHeld: debug.authorityMetadataHeld,
        authorityVolumeHeld: debug.authorityVolumeHeld,
        authorityAppBundle: debug.authorityAppBundle,
        arbitratedSource: debug.arbitratedSource,
        librarySource: debug.librarySource,
        lastPlayedFrom: debug.lastPlayedFrom,
        attachedProviders: debug.attachedProviders,
        attachedSchemes: debug.attachedSchemes,
        linkedDevices: debug.linkedDevices,
        autoResume: debug.autoResume.map {
            BridgethingDeviceAutoResume(deviceId: $0.deviceId, enabled: $0.enabled)
        },
        voice: BridgethingVoiceDebug(
            hasModel: debug.voice.hasModel,
            armedBundle: debug.voice.armedBundle,
            transferAllowed: debug.voice.transferAllowed,
            nluBundleDir: debug.voice.paths.nluBundleDir,
            asrWeights: debug.voice.paths.asrWeights
        )
    )
}

private func toRNVoiceTurn(_ turn: VoiceTurn) -> BridgethingVoiceTurn {
    let trigger: BridgethingVoiceTurnTrigger = switch turn.trigger {
    case .pushToTalk: .pushtotalk
    case .assistant: .assistant
    case .wakeWord: .wakeword
    }
    let phase: BridgethingVoiceTurnPhase = switch turn.phase {
    case .listening: .listening
    case .resolved: .resolved
    case .cancelled: .cancelled
    }
    return BridgethingVoiceTurn(
        deviceId: turn.deviceId,
        streamId: turn.streamId,
        trigger: trigger,
        phase: phase,
        transcript: turn.transcript,
        intent: turn.intent
    )
}

private func toRNVoiceModelState(_ state: VoiceModelState) -> BridgethingVoiceModelState {
    let status: BridgethingVoiceModelStatus = switch state.status {
    case .absent: .absent
    case .downloading: .downloading
    case .ready: .ready
    case .failed: .failed
    }
    return BridgethingVoiceModelState(
        status: status,
        receivedBytes: Double(state.receivedBytes),
        totalBytes: Double(state.totalBytes),
        version: state.version,
        error: state.error
    )
}

private func toRNWebappsEntry(_ entry: DeviceWebappsEntry) -> BridgethingDeviceWebappsEntry {
    BridgethingDeviceWebappsEntry(
        deviceId: entry.deviceId,
        webapps: visible(entry.webapps).map(toRNWebappInfo),
        active: entry.active.map(toRNActiveWebapp)
    )
}

private func toRNActiveWebapp(_ active: ActiveWebapp) -> BridgethingActiveWebapp {
    BridgethingActiveWebapp(id: active.id, name: active.name)
}

private func toRNWebappInfo(_ info: WebappInfo) -> BridgethingWebappInfo {
    BridgethingWebappInfo(
        id: info.id,
        name: info.name,
        source: info.source == .builtin ? .builtin : .installed,
        role: info.role == .launcher ? .launcher : .standard,
        version: info.version,
        provenance: info.provenance,
        description: info.description,
        iconHash: info.iconHash,
        settingsHash: info.settingsHash,
        overlayHash: info.overlayHash,
        config: info.config.map(toRNConfigField),
        permissions: info.permissions
    )
}

private func toRNWebappSlots(_ slots: WebappSlots) -> BridgethingWebappSlots {
    BridgethingWebappSlots(launcher: slots.launcher, overlay: slots.overlay)
}

private func toCoreWebappSlot(_ slot: BridgethingWebappSlot) -> WebappSlot {
    slot == .launcher ? .launcher : .overlay
}

private func toRNConfigField(_ field: ConfigField) -> BridgethingConfigField {
    let kind: BridgethingConfigKind = switch field.kind {
    case .string: .string
    case .secret: .secret
    case .number: .number
    case .boolean: .boolean
    case .enum: .enum
    }
    return BridgethingConfigField(
        kind: kind,
        key: field.key,
        label: field.label,
        pattern: field.pattern,
        minLength: field.minLength.map(Double.init),
        maxLength: field.maxLength.map(Double.init),
        min: field.min,
        max: field.max,
        step: field.step,
        choices: field.kind == .enum ? field.choices : nil,
        defaultValue: field.defaultValue
    )
}

private func toRNOtaManifest(_ m: OtaDiscoverManifest) -> BridgethingOtaManifest {
    let channels = m.channels.map { (slug, ch) -> BridgethingOtaChannelInfo in
        let releases = ch.releases.compactMap { v -> BridgethingOtaRelease? in
            guard let composite = parseOtaCompositeVersion(raw: v) else { return nil }
            let rel = m.releases[v]
            return BridgethingOtaRelease(
                version: v,
                daemonVersion: composite.daemon,
                imageVersion: composite.image,
                yanked: rel?.yanked != nil,
                deprecated: rel?.deprecated ?? false
            )
        }
        return BridgethingOtaChannelInfo(
            slug: slug,
            name: ch.name,
            stability: ch.stability,
            isDefault: ch.isDefault,
            latest: ch.latest,
            releases: releases
        )
    }
    return BridgethingOtaManifest(updatedAt: m.updatedAt, channels: channels)
}

private func toRNOtaRun(_ run: OtaRun) -> BridgethingOtaRun {
    BridgethingOtaRun(
        runId: run.runId,
        deviceId: run.deviceId,
        otaKind: toRNOtaKind(run.kind),
        phase: toRNOtaRunPhase(run.phase),
        steps: run.steps.map {
            BridgethingOtaStep(id: Double($0.id), kind: toRNOtaStepKind($0.kind), label: $0.label, bytes: Double($0.bytes))
        },
        stepId: Double(run.stepId),
        startedAt: Double(run.startedAtMs),
        phaseStartedAt: Double(run.phaseStartedAtMs),
        stageReceived: run.stageReceived.map(Double.init),
        stageTotal: run.stageTotal.map(Double.init),
        ratePerSec: run.ratePerSec,
        dwlPercent: run.dwlPercent.map(Double.init),
        outcome: run.outcome.map(toRNOtaOutcome),
        error: run.error,
        releaseVersion: run.releaseVersion,
        daemonVersion: run.daemonVersion,
        imageVersion: run.imageVersion,
        resumable: run.resumable,
        webappId: run.webappId,
        webappName: run.webappName
    )
}

private func toRNOtaProgress(_ progress: OtaRunProgress) -> BridgethingOtaProgress {
    BridgethingOtaProgress(
        percent: Double(progress.percent),
        stepIndex: Double(progress.stepIndex),
        stepCount: Double(progress.stepCount),
        stepLabel: progress.stepLabel,
        etaSeconds: progress.etaSeconds.map(Double.init)
    )
}

private func toRNOtaRunPhase(_ p: OtaRunPhase) -> BridgethingOtaPhase {
    switch p {
    case .idle: .idle
    case .downloading: .downloading
    case .streaming: .streaming
    case .verifying: .verifying
    case .writing: .writing
    case .confirming: .confirming
    case .reboot: .reboot
    case .completed: .completed
    case .failed: .failed
    }
}

private func toRNOtaKind(_ k: OtaKind) -> BridgethingOtaKind {
    switch k {
    case .image: .image
    case .daemon: .daemon
    case .builtinWebapp: .builtinwebapp
    case .installedWebapp: .installedwebapp
    case .wakewordModel: .wakewordmodel
    }
}

private func toRNOtaStepKind(_ k: OtaStepKind) -> BridgethingOtaStepKind {
    switch k {
    case .download: .download
    case .stream: .stream
    case .apply: .apply
    case .reboot: .reboot
    }
}

private func toRNOtaOutcome(_ o: OtaRunOutcome) -> BridgethingOtaOutcome {
    switch o {
    case .succeeded: .succeeded
    case .failed: .failed
    case .cancelled: .cancelled
    }
}

private func toRNOtaAvailable(_ a: OtaAvailable) -> BridgethingOtaAvailable {
    BridgethingOtaAvailable(
        deviceId: a.deviceId,
        releaseVersion: a.releaseVersion,
        daemonVersion: a.daemonVersion,
        imageVersion: a.imageVersion
    )
}

private func toRNOtaPollStatus(_ s: OtaPollStatus) -> BridgethingOtaPollStatus {
    BridgethingOtaPollStatus(lastPolledAt: s.lastPolledAt, error: s.error)
}

private func toRNOtaPollConfig(_ config: OtaPollConfig) -> BridgethingOtaPollConfig {
    BridgethingOtaPollConfig(
        intervalSeconds: Double(config.intervalSeconds),
        autoPush: config.autoPush,
        rootUrl: config.rootUrl
    )
}

private func toCoreOtaPollConfig(_ config: BridgethingOtaPollConfig) -> OtaPollConfig {
    OtaPollConfig(
        intervalSeconds: UInt64(max(60, config.intervalSeconds)),
        autoPush: config.autoPush,
        rootUrl: config.rootUrl
    )
}

private func toCoreResumeTarget(_ target: BridgethingResumeTarget) -> ResumeTarget {
    switch target {
    case .phoneonly: .phoneOnly
    case .anyspeaker: .anySpeaker
    }
}

private let daemonLabel = "daemon"

private func toOriginName(_ origin: BridgethingCompanionCore.LogOrigin) -> String {
    switch origin {
    case .device: "device"
    case .host: "local"
    }
}

private func toLevelName(_ level: BridgethingCompanionCore.LogLevel) -> String {
    switch level {
    case .trace, .debug: "debug"
    case .info: "info"
    case .warn: "warn"
    case .error: "error"
    }
}

private func toLevelName(_ level: LogStoreLevel) -> String {
    switch level {
    case .trace, .debug: "debug"
    case .info, .notice: "info"
    case .warn: "warn"
    case .error, .fatal: "error"
    }
}

private func toStoreLevel(_ level: BridgethingCompanionCore.LogLevel) -> LogStoreLevel {
    switch level {
    case .trace, .debug: .debug
    case .info: .info
    case .warn: .warn
    case .error: .error
    }
}
