#if os(iOS)

    import BridgethingCompanionCore
    import ExternalAccessory
    import Foundation
    import Logging
    import UIKit

    private let shellLog = Logger(label: "com.bridgething.companion.shell")

    private final class EventRelay: SessionEventSink, @unchecked Sendable {
        private let lock = NSLock()
        private var handler: (@Sendable (SessionEvent) -> Void)?

        func bind(_ handler: @escaping @Sendable (SessionEvent) -> Void) {
            lock.lock()
            self.handler = handler
            lock.unlock()
        }

        func onEvent(event: SessionEvent) {
            lock.lock()
            let held = handler
            lock.unlock()
            held?(event)
        }
    }

    private final class SessionRef: @unchecked Sendable {
        private let lock = NSLock()
        private weak var session: CompanionSession?

        var current: CompanionSession? {
            get {
                lock.lock()
                defer { lock.unlock() }
                return session
            }
            set {
                lock.lock()
                session = newValue
                lock.unlock()
            }
        }
    }

    public final class BridgethingCompanion: @unchecked Sendable {
        public let transport: EALinkTransport

        public let session: CompanionSession

        private let events: @Sendable (SessionEvent) -> Void
        private let keepAlive = BackgroundAudioKeepAlive()

        private let lock = NSLock()
        private var connectedIds: Set<String> = []
        private var serials: [String: String] = [:]
        private var ancsStatuses: [String: AncsAuthStatus] = [:]
        private var ancsCoordinator: AncsPairCoordinator?
        private var ancsPromotionInFlight = false
        private var observers: [NSObjectProtocol] = []

        public init(
            host: HostInfo,
            capabilities: CapabilityFlags,
            spotify: SpotifyProviderConfig?,
            eaProtocolString: String = "com.bridgething.gateway",
            earconBundle: Bundle = .main,
            events: @escaping @Sendable (SessionEvent) -> Void
        ) {
            self.events = events
            transport = EALinkTransport(protocolString: eaProtocolString)
            let secrets = KeychainSecretStore()
            let sessionRef = SessionRef()
            let relay = EventRelay()
            session = CompanionSession.create(
                config: CompanionConfig(
                    host: host,
                    capabilities: capabilities,
                    stateDir: Self.directory(.applicationSupportDirectory),
                    cacheDir: Self.directory(.cachesDirectory),
                    modelPlatform: .ios,
                    spotify: spotify
                ),
                backends: CompanionBackends(
                    link: transport,
                    host: FoundationHostEnvironment(),
                    http: UrlSessionHttpTransport(),
                    ws: UrlSessionWsTransport(),
                    secrets: secrets,
                    log: OSLogSink(),
                    audio: AvAudioBackend(earconBundle: earconBundle),
                    volume: nil,
                    geo: CoreLocationGeoProvider(),
                    notifications: nil,
                    phone: nil,
                    mediaSessions: nil,
                    stream: StreamPlayer(),
                    speech: SpeechTranscriberBackend(),
                    nlu: CoreMlNluRunner { sessionRef.current?.voiceModelPaths().nluBundleDir },
                    appleMusic: MusicKitBackend(),
                    image: ImageIoScaler(),
                    modelValidator: CoreMlArtifactValidator(),
                    transferPolicy: UnmeteredTransferPolicy(),
                    connectivity: NwPathConnectivityMonitor(),
                    deviceWaker: nil
                ),
                events: relay
            )
            sessionRef.current = session
            relay.bind { [weak self] event in self?.handleEvent(event) }
            let center = NotificationCenter.default
            observers.append(
                center.addObserver(
                    forName: UIApplication.didBecomeActiveNotification, object: nil, queue: nil
                ) { [weak self] _ in
                    Task { await self?.ensureAncsPairing() }
                }
            )
            for name in [Notification.Name.NSSystemTimeZoneDidChange, .NSSystemClockDidChange] {
                observers.append(
                    center.addObserver(forName: name, object: nil, queue: nil) { [weak self] _ in
                        Task { await self?.timeChanged() }
                    }
                )
            }
        }

        deinit {
            for token in observers { NotificationCenter.default.removeObserver(token) }
        }

        public func start() async throws {
            try await session.start()
        }

        public func stop() async {
            await session.stop()
            await keepAlive.deactivate()
            lock.withLock { connectedIds.removeAll() }
        }

        public func resumed() async {
            await session.resumed()
        }

        public func timeChanged() async {
            await session.timeChanged()
        }

        public func logInbox() -> LogInbox {
            session.logInbox()
        }

        // MARK: - ancs pairing

        public func currentAncsAuthState(deviceId: String) -> AncsAuthStatus {
            lock.lock()
            defer { lock.unlock() }
            return ancsStatuses[deviceId] ?? .unknown
        }

        public func enableAncsNotifications(deviceId: String) async -> AncsSetupResult {
            guard let serial = await resolveSerial(deviceId: deviceId) else {
                return AncsSetupResult(kind: .failed("no metadata for device \(deviceId)"), authState: .unknown)
            }
            let coordinator = await makeOrReuseCoordinator()
            await coordinator.setAuthState(serial: serial, currentAncsAuthState(deviceId: deviceId))
            let result = await coordinator.pair(serial: serial)
            shellLog.info("enableAncsNotifications: result \(String(describing: result.kind))")
            return result
        }

        private func handleEvent(_ event: SessionEvent) {
            switch event {
            case let .peerConnected(peer):
                lock.lock()
                let wasEmpty = connectedIds.isEmpty
                connectedIds.insert(peer.id)
                lock.unlock()
                if wasEmpty {
                    Task { [keepAlive] in await keepAlive.activate() }
                }
                Task { [weak self] in await self?.ensureAncsPairing() }
            case let .peerDisconnected(deviceId):
                notePeerGone(deviceId)
            case let .peerLinkFailed(peer):
                notePeerGone(peer.id)
            case let .deviceMetaChanged(deviceId, meta):
                if !meta.serialNumber.isEmpty {
                    lock.lock()
                    serials[deviceId] = meta.serialNumber
                    lock.unlock()
                    Task { [weak self] in await self?.ensureAncsPairing() }
                }
            case let .ancsAuthStatusChanged(deviceId, status):
                lock.lock()
                ancsStatuses[deviceId] = status
                let serial = serials[deviceId]
                let coordinator = ancsCoordinator
                lock.unlock()
                if let serial, let coordinator {
                    Task { await coordinator.setAuthState(serial: serial, status) }
                }
            default:
                break
            }
            events(event)
        }

        private func notePeerGone(_ deviceId: String) {
            lock.lock()
            let known = connectedIds.remove(deviceId) != nil
            let empty = connectedIds.isEmpty
            lock.unlock()
            if known, empty {
                Task { [keepAlive] in await keepAlive.deactivate() }
            }
        }

        private func resolveSerial(deviceId: String) async -> String? {
            if let cached = lock.withLock({ serials[deviceId] }) { return cached }
            let meta = await session.snapshot().deviceMeta.first { $0.deviceId == deviceId }?.meta
            guard let serial = meta?.serialNumber, !serial.isEmpty else { return nil }
            lock.withLock { serials[deviceId] = serial }
            return serial
        }

        private func makeOrReuseCoordinator() async -> AncsPairCoordinator {
            if let existing = lock.withLock({ ancsCoordinator }) { return existing }
            let fresh = await MainActor.run { AncsPairCoordinator() }
            return lock.withLock {
                if let existing = ancsCoordinator { return existing }
                ancsCoordinator = fresh
                return fresh
            }
        }

        private func ensureAncsPairing() async {
            let (ids, alreadyRunning) = lock.withLock { () -> (Set<String>, Bool) in
                let ids = connectedIds
                let running = ancsPromotionInFlight
                if !running, !ids.isEmpty { ancsPromotionInFlight = true }
                return (ids, running)
            }
            guard !alreadyRunning, !ids.isEmpty else { return }
            defer {
                lock.withLock { ancsPromotionInFlight = false }
            }
            let coordinator = await makeOrReuseCoordinator()
            for id in ids {
                guard let serial = await resolveSerial(deviceId: id) else { continue }
                await coordinator.setAuthState(serial: serial, currentAncsAuthState(deviceId: id))
                if await coordinator.hasPairedAccessory(serial: serial) {
                    await coordinator.reconnectIfPaired(serial: serial)
                    continue
                }
                let result = await coordinator.pair(serial: serial)
                shellLog.info(
                    "ancs promotion \(serial): \(String(describing: result.kind)) (auth \(String(describing: result.authState)))"
                )
            }
        }

        // MARK: - ea pair picker

        public func presentPairPicker() async -> AccessoryPickResult? {
            await withCheckedContinuation { (cont: CheckedContinuation<AccessoryPickResult?, Never>) in
                Task { @MainActor in
                    shellLog.info("presenting EA bluetooth accessory picker")
                    EAAccessoryManager.shared().showBluetoothAccessoryPicker(withNameFilter: nil) { error in
                        guard let error else {
                            shellLog.info("EA picker completed")
                            cont.resume(returning: AccessoryPickResult(id: "", name: AncsBluetooth.productLabel))
                            return
                        }
                        let ns = error as NSError
                        if ns.domain == EABluetoothAccessoryPickerErrorDomain,
                            let code = EABluetoothAccessoryPickerError.Code(rawValue: ns.code)
                        {
                            switch code {
                            case .alreadyConnected:
                                shellLog.info("EA picker: accessory already connected")
                                cont.resume(returning: AccessoryPickResult(id: "", name: AncsBluetooth.productLabel))
                                return
                            case .resultNotFound:
                                shellLog.warning("EA picker: no accessory found")
                            case .resultCancelled:
                                shellLog.warning("EA picker dismissed without pairing")
                            case .resultFailed:
                                shellLog.warning("EA picker: pairing failed")
                            @unknown default:
                                shellLog.warning("EA picker error: \(error.localizedDescription)")
                            }
                        } else {
                            shellLog.warning("EA picker error: \(error.localizedDescription)")
                        }
                        cont.resume(returning: nil)
                    }
                }
            }
        }

        private static func directory(_ base: FileManager.SearchPathDirectory) -> String {
            let url = FileManager.default.urls(for: base, in: .userDomainMask)[0]
                .appendingPathComponent("bridgething", isDirectory: true)
            try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
            return url.path
        }
    }

#endif
