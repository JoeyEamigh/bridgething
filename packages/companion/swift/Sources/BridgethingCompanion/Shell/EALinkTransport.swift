#if canImport(ExternalAccessory)

    import BridgethingCompanionCore
    import ExternalAccessory
    import Foundation
    import os

    private let eaLog = Logger(subsystem: "com.bridgething.companion", category: "ea")

    private final class EAIOThread {
        private let thread: Thread
        private let cfLoop: CFRunLoop

        init() {
            let ready = DispatchSemaphore(value: 0)
            let box = UnsafeMutablePointer<CFRunLoop?>.allocate(capacity: 1)
            box.initialize(to: nil)
            thread = Thread {
                box.pointee = CFRunLoopGetCurrent()
                ready.signal()
                RunLoop.current.add(NSMachPort(), forMode: .default)
                while !Thread.current.isCancelled {
                    RunLoop.current.run(mode: .default, before: .distantFuture)
                }
            }
            thread.name = "bridgething-ea-io"
            thread.qualityOfService = .userInitiated
            thread.start()
            ready.wait()
            cfLoop = box.pointee!
            box.deinitialize(count: 1)
            box.deallocate()
        }

        func perform(_ block: @escaping () -> Void) {
            CFRunLoopPerformBlock(cfLoop, CFRunLoopMode.defaultMode.rawValue, block)
            CFRunLoopWakeUp(cfLoop)
        }

        func stop() {
            thread.cancel()
            CFRunLoopWakeUp(cfLoop)
        }
    }

    public final class EALinkTransport: LinkTransport, @unchecked Sendable {
        private let protocolString: String
        private let retryAttemptsBeforeAnnounce = 6
        private let retryBaseInterval = 1.0
        private let retryMaxInterval = 30.0

        private let lock = NSLock()
        private var inbox: LinkInbox?
        private var ioThread: EAIOThread?
        private var sessions: [String: EASessionState] = [:]
        private var linkedUp: Set<String> = []
        private var retryTasks: [String: Task<Void, Never>] = [:]
        private var linkFailedReported: Set<String> = []
        private var observers: [NSObjectProtocol] = []
        private var started = false

        public init(protocolString: String = "com.bridgething.gateway") {
            self.protocolString = protocolString
        }

        public func maxBatchBytes() -> UInt32 { 32 * 1024 }

        public func start(inbox: LinkInbox) {
            lock.lock()
            guard !started else {
                lock.unlock()
                return
            }
            started = true
            self.inbox = inbox
            ioThread = EAIOThread()
            lock.unlock()

            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                let center = NotificationCenter.default
                let connect = center.addObserver(
                    forName: .EAAccessoryDidConnect, object: nil, queue: .main
                ) { [weak self] note in
                    guard let accessory = note.userInfo?[EAAccessoryKey] as? EAAccessory else { return }
                    self?.tryOpenSession(for: accessory)
                }
                let disconnect = center.addObserver(
                    forName: .EAAccessoryDidDisconnect, object: nil, queue: .main
                ) { [weak self] note in
                    guard let accessory = note.userInfo?[EAAccessoryKey] as? EAAccessory else { return }
                    self?.handleAccessoryGone(deviceId: Self.deviceId(for: accessory))
                }
                self.lock.lock()
                self.observers = [connect, disconnect]
                self.lock.unlock()
                EAAccessoryManager.shared().registerForLocalNotifications()
                for accessory in EAAccessoryManager.shared().connectedAccessories {
                    self.tryOpenSession(for: accessory)
                }
            }
        }

        public func stop() {
            lock.lock()
            guard started else {
                lock.unlock()
                return
            }
            started = false
            inbox = nil
            let held = observers
            observers = []
            let tasks = retryTasks
            retryTasks = [:]
            linkFailedReported = []
            let open = sessions
            sessions = [:]
            linkedUp = []
            let io = ioThread
            ioThread = nil
            lock.unlock()

            for observer in held { NotificationCenter.default.removeObserver(observer) }
            DispatchQueue.main.async { EAAccessoryManager.shared().unregisterForLocalNotifications() }
            for (_, task) in tasks { task.cancel() }
            for (_, session) in open { io?.perform { session.tearDown() } }
            io?.stop()
        }

        public func send(deviceId: String, batch: Data) {
            lock.lock()
            let session = linkedUp.contains(deviceId) ? sessions[deviceId] : nil
            let io = ioThread
            let held = inbox
            lock.unlock()
            guard let session, let io else {
                held?.onSendFailed(deviceId: deviceId)
                return
            }
            io.perform { session.enqueue(batch) }
        }

        public func disconnect(deviceId: String) {
            lock.lock()
            retryTasks.removeValue(forKey: deviceId)?.cancel()
            linkFailedReported.remove(deviceId)
            linkedUp.remove(deviceId)
            let session = sessions.removeValue(forKey: deviceId)
            let io = ioThread
            let held = inbox
            lock.unlock()
            guard let session else { return }
            io?.perform { session.tearDown() }
            held?.onDisconnected(deviceId: deviceId)
        }

        public func reconnect(deviceId: String) {
            lock.lock()
            retryTasks.removeValue(forKey: deviceId)?.cancel()
            linkFailedReported.remove(deviceId)
            linkedUp.remove(deviceId)
            let session = sessions.removeValue(forKey: deviceId)
            let io = ioThread
            lock.unlock()
            if let session { io?.perform { session.tearDown() } }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                guard let accessory = EAAccessoryManager.shared().connectedAccessories
                    .first(where: { Self.deviceId(for: $0) == deviceId })
                else { return }
                self.tryOpenSession(for: accessory)
            }
        }

        // MARK: - session lifecycle

        fileprivate func handleInbound(deviceId: String, bytes: Data) {
            currentInbox()?.onBytes(deviceId: deviceId, bytes: bytes)
        }

        fileprivate func handleWriteComplete(deviceId: String) {
            currentInbox()?.onWriteComplete(deviceId: deviceId)
        }

        fileprivate func linkUp(_ state: EASessionState) {
            let id = state.deviceId
            lock.lock()
            guard sessions[id] === state else {
                lock.unlock()
                return
            }
            linkedUp.insert(id)
            retryTasks.removeValue(forKey: id)?.cancel()
            linkFailedReported.remove(id)
            let held = inbox
            lock.unlock()
            eaLog.info("ea link up for \(id, privacy: .public)")
            held?.onConnected(device: LinkDevice(id: id, name: state.accessory.name))
        }

        fileprivate func linkOpenFailed(_ state: EASessionState, reason: String) {
            let id = state.deviceId
            eaLog.warning("ea open failed for \(id, privacy: .public) (attempt \(state.attempt + 1)): \(reason, privacy: .public)")
            lock.lock()
            if sessions[id] === state { sessions.removeValue(forKey: id) }
            let io = ioThread
            lock.unlock()
            io?.perform { state.tearDown() }
            scheduleRetryOrFail(accessory: state.accessory, attempt: state.attempt, reason: reason)
        }

        fileprivate func linkDropped(_ state: EASessionState, reason: String) {
            let id = state.deviceId
            lock.lock()
            guard sessions[id] === state else {
                lock.unlock()
                return
            }
            sessions.removeValue(forKey: id)
            linkedUp.remove(id)
            let io = ioThread
            let held = inbox
            lock.unlock()
            io?.perform { state.tearDown() }
            held?.onSendFailed(deviceId: id)
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                let stillAttached = EAAccessoryManager.shared().connectedAccessories
                    .contains { Self.deviceId(for: $0) == id }
                if stillAttached {
                    eaLog.warning("ea link dropped for \(id, privacy: .public) after link-up (\(reason, privacy: .public)); re-opening")
                    self.scheduleRetryOrFail(accessory: state.accessory, attempt: 0, reason: reason)
                } else {
                    eaLog.info("ea link ended for \(id, privacy: .public) (\(reason, privacy: .public)); accessory gone")
                    self.lock.lock()
                    self.linkFailedReported.remove(id)
                    self.lock.unlock()
                    held?.onDisconnected(deviceId: id)
                }
            }
        }

        private func handleAccessoryGone(deviceId: String) {
            lock.lock()
            retryTasks.removeValue(forKey: deviceId)?.cancel()
            linkFailedReported.remove(deviceId)
            linkedUp.remove(deviceId)
            let session = sessions.removeValue(forKey: deviceId)
            let io = ioThread
            let held = inbox
            lock.unlock()
            guard let session else { return }
            io?.perform { session.tearDown() }
            held?.onDisconnected(deviceId: deviceId)
        }

        private func tryOpenSession(for accessory: EAAccessory, attempt: Int = 0) {
            guard accessory.protocolStrings.contains(protocolString) else { return }
            let id = Self.deviceId(for: accessory)
            lock.lock()
            guard started, sessions[id] == nil, let io = ioThread else {
                lock.unlock()
                return
            }
            guard let session = EASession(accessory: accessory, forProtocol: protocolString) else {
                lock.unlock()
                scheduleRetryOrFail(
                    accessory: accessory, attempt: attempt,
                    reason: "EASession(accessory:forProtocol:) returned nil"
                )
                return
            }
            let state = EASessionState(accessory: accessory, session: session, owner: self, attempt: attempt)
            sessions[id] = state
            lock.unlock()
            io.perform { state.openStreams() }
        }

        private func scheduleRetryOrFail(accessory: EAAccessory, attempt: Int, reason: String) {
            let id = Self.deviceId(for: accessory)
            let exhausted = attempt + 1 >= retryAttemptsBeforeAnnounce
            lock.lock()
            let announce = exhausted && linkFailedReported.insert(id).inserted
            let held = inbox
            let delay = min(retryBaseInterval * pow(2.0, Double(attempt)), retryMaxInterval)
            let nextAttempt = exhausted ? attempt : attempt + 1
            retryTasks[id]?.cancel()
            let task = Task { [weak self] in
                try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
                guard let self, !Task.isCancelled else { return }
                self.lock.withLock { _ = self.retryTasks.removeValue(forKey: id) }
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    guard let next = EAAccessoryManager.shared().connectedAccessories
                        .first(where: { Self.deviceId(for: $0) == id })
                    else {
                        self.lock.lock()
                        self.linkFailedReported.remove(id)
                        self.lock.unlock()
                        return
                    }
                    self.tryOpenSession(for: next, attempt: nextAttempt)
                }
            }
            retryTasks[id] = task
            lock.unlock()
            if announce {
                eaLog.error("link failed for \(id, privacy: .public) after \(attempt + 1) attempts: \(reason, privacy: .public); continuing slow retry")
                held?.onLinkFailed(deviceId: id, name: accessory.name, reason: reason)
            }
        }

        private func currentInbox() -> LinkInbox? {
            lock.lock()
            defer { lock.unlock() }
            return inbox
        }

        private static func deviceId(for accessory: EAAccessory) -> String {
            let serial = accessory.serialNumber
            return serial.isEmpty ? "ea-\(accessory.connectionID)" : serial
        }
    }

    private final class EASessionState: NSObject, StreamDelegate, @unchecked Sendable {
        let accessory: EAAccessory
        let session: EASession
        let attempt: Int
        weak var owner: EALinkTransport?

        private var pendingBatches: [Data] = []
        private var currentWrite = Data()
        private var inputOpen = false
        private var outputOpen = false
        private var isLinkedUp = false
        private var openFailed = false
        private var readBuffer = [UInt8](repeating: 0, count: 64 * 1024)

        var deviceId: String {
            let serial = accessory.serialNumber
            return serial.isEmpty ? "ea-\(accessory.connectionID)" : serial
        }

        init(accessory: EAAccessory, session: EASession, owner: EALinkTransport, attempt: Int) {
            self.accessory = accessory
            self.session = session
            self.owner = owner
            self.attempt = attempt
            super.init()
        }

        func openStreams() {
            if let input = session.inputStream {
                input.delegate = self
                input.schedule(in: .current, forMode: .default)
                input.open()
            }
            if let output = session.outputStream {
                output.delegate = self
                output.schedule(in: .current, forMode: .default)
                output.open()
            }
        }

        func tearDown() {
            if let input = session.inputStream {
                input.close()
                input.remove(from: .current, forMode: .default)
                input.delegate = nil
            }
            if let output = session.outputStream {
                output.close()
                output.remove(from: .current, forMode: .default)
                output.delegate = nil
            }
        }

        func enqueue(_ batch: Data) {
            pendingBatches.append(batch)
            drainOutput()
        }

        /// iOS's External Accessory stack degrades sharply (and can drop the link)
        /// when a single OutputStream.write exceeds ~2KB. Cap each write and let
        /// the drain loop split larger batches across multiple writes; the
        /// protocol framing above is untouched.
        private static let maxWriteBytes = 2048

        private func drainOutput() {
            guard let out = session.outputStream else { return }
            while out.hasSpaceAvailable {
                if currentWrite.isEmpty {
                    guard !pendingBatches.isEmpty else { return }
                    currentWrite = pendingBatches.removeFirst()
                }
                let written = currentWrite.withUnsafeBytes { raw -> Int in
                    guard let base = raw.bindMemory(to: UInt8.self).baseAddress else { return 0 }
                    return out.write(base, maxLength: min(currentWrite.count, Self.maxWriteBytes))
                }
                if written < 0 {
                    eaLog.warning("ea write error for \(self.deviceId, privacy: .public): \(String(describing: out.streamError), privacy: .public); dropping link")
                    owner?.linkDropped(self, reason: "write error")
                    return
                }
                if written <= 0 { return }
                currentWrite.removeSubrange(0 ..< written)
                if currentWrite.isEmpty {
                    owner?.handleWriteComplete(deviceId: deviceId)
                }
            }
        }

        func stream(_ aStream: Stream, handle eventCode: Stream.Event) {
            switch eventCode {
            case .openCompleted:
                if aStream === session.inputStream { inputOpen = true }
                if aStream === session.outputStream { outputOpen = true }
                if inputOpen, outputOpen, !isLinkedUp {
                    isLinkedUp = true
                    owner?.linkUp(self)
                }
            case .hasBytesAvailable:
                guard let input = aStream as? InputStream else { return }
                var drained = Data()
                while input.hasBytesAvailable {
                    let n = readBuffer.withUnsafeMutableBufferPointer { ptr -> Int in
                        guard let base = ptr.baseAddress else { return 0 }
                        return input.read(base, maxLength: ptr.count)
                    }
                    if n < 0 {
                        eaLog.warning("ea read error for \(self.deviceId, privacy: .public): \(String(describing: input.streamError), privacy: .public); dropping link")
                        owner?.linkDropped(self, reason: "read error")
                        return
                    }
                    if n <= 0 { break }
                    drained.append(contentsOf: readBuffer[0 ..< n])
                }
                if !drained.isEmpty {
                    owner?.handleInbound(deviceId: deviceId, bytes: drained)
                }
            case .hasSpaceAvailable:
                drainOutput()
            case .endEncountered, .errorOccurred:
                if isLinkedUp {
                    let reason = eventCode == .errorOccurred ? "stream error after link-up" : "stream closed after link-up"
                    owner?.linkDropped(self, reason: reason)
                } else if !openFailed {
                    openFailed = true
                    let reason = eventCode == .errorOccurred ? "stream error during open" : "stream closed during open"
                    owner?.linkOpenFailed(self, reason: reason)
                }
            default:
                break
            }
        }
    }

#endif
