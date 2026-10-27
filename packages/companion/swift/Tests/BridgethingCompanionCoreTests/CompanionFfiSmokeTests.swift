import BridgethingCompanionCore
import Foundation
import XCTest

private let smokeMaxBatchBytes: UInt32 = 32768
private let smokeDeviceId = "swift-smoke-device"
private let smokeDeviceName = "smoke device"
private let smokeTimeout: TimeInterval = 10

private final class RecordingSecretStore: SecretStore, @unchecked Sendable {
    func get(key: String) -> String? { nil }
    func set(key: String, value: String) {}
    func remove(key: String) {}
    func getBlob(key: String) -> Data? { nil }
}

private final class RecordingEventSink: SessionEventSink, @unchecked Sendable {
    private let condition = NSCondition()
    private var events: [SessionEvent] = []

    func onEvent(event: SessionEvent) {
        condition.lock()
        events.append(event)
        condition.broadcast()
        condition.unlock()
    }

    func waitFor<T>(_ match: (SessionEvent) -> T?) -> T? {
        let deadline = Date().addingTimeInterval(smokeTimeout)
        var next = 0
        condition.lock()
        defer { condition.unlock() }
        while true {
            while next < events.count {
                if let hit = match(events[next]) { return hit }
                next += 1
            }
            if !condition.wait(until: deadline) { return nil }
        }
    }
}

private final class LoopbackLinkTransport: LinkTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var batches: [Data] = []
    private var inbox: LinkInbox?

    func maxBatchBytes() -> UInt32 { smokeMaxBatchBytes }

    func start(inbox: LinkInbox) {
        lock.lock()
        self.inbox = inbox
        lock.unlock()
        inbox.onConnected(device: LinkDevice(id: smokeDeviceId, name: smokeDeviceName))
    }

    func stop() {
        lock.lock()
        inbox = nil
        lock.unlock()
    }

    func send(deviceId: String, batch: Data) {
        lock.lock()
        batches.append(batch)
        let live = inbox
        lock.unlock()
        live?.onWriteComplete(deviceId: deviceId)
    }

    func disconnect(deviceId: String) {}

    func reconnect(deviceId: String) {}

    func waitForBatch() -> Data? {
        let deadline = Date().addingTimeInterval(smokeTimeout)
        while Date() < deadline {
            lock.lock()
            let first = batches.first
            lock.unlock()
            if let first { return first }
            Thread.sleep(forTimeInterval: 0.02)
        }
        return nil
    }
}

private final class FixedHost: HostEnvironment, @unchecked Sendable {
    func clock() -> HostClock {
        HostClock(
            tzIana: "UTC", locale: "en-US", unixSeconds: 1_700_000_000,
            utcOffsetMinutes: 0, dstOffsetMinutes: 0
        )
    }
}

private final class OfflineHttpTransport: HttpTransport, @unchecked Sendable {
    func execute(request: HttpRequest, sink: HttpSink) {
        sink.fail(reason: "the smoke test has no network")
    }

    func download(request: HttpRequest, sink: HttpDownloadSink) {
        sink.onFailed(reason: "the smoke test has no network")
    }
}

private final class OfflineWsTransport: WsTransport, @unchecked Sendable {
    func connect(connect: WsConnect, inbox: WsInbox) {
        inbox.onClosed(id: connect.id, code: nil, reason: "the smoke test has no network")
    }

    func send(id: String, frame: WsFrame) {}

    func disconnect(id: String, code: UInt16?, reason: String?) {}
}

private final class CollectingLogSink: LogSink, @unchecked Sendable {
    func onLine(level: LogLevel, target: String, message: String) {}
}

private func scratch() -> String {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("companion-smoke-\(UUID().uuidString)")
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir.path
}

private func config(_ stateDir: String) -> CompanionConfig {
    CompanionConfig(
        host: HostInfo(
            appName: "smoke", appVersion: "0.0.0", osName: "linux", osVersion: "test",
            hostIdentifier: "swift-smoke"
        ),
        capabilities: CapabilityFlags(
            geo: false, notifications: false, netFetch: false, netWs: false,
            audioTts: false, voiceModel: false
        ),
        stateDir: stateDir,
        cacheDir: stateDir
    )
}

private func backends(_ link: LinkTransport) -> CompanionBackends {
    CompanionBackends(
        link: link, host: FixedHost(), http: OfflineHttpTransport(), ws: OfflineWsTransport(),
        secrets: RecordingSecretStore(), log: CollectingLogSink()
    )
}

final class CompanionFfiSmokeTests: XCTestCase {
    func testStartBringsUpTheLinkAndTellsTheHostAboutThePeer() async throws {
        let link = LoopbackLinkTransport()
        let sink = RecordingEventSink()
        let session = CompanionSession.create(config: config(scratch()), backends: backends(link), events: sink)

        try await session.start()

        let peer = try XCTUnwrap(sink.waitFor { event -> SessionPeer? in
            guard case let .peerConnected(peer) = event else { return nil }
            return peer
        })
        XCTAssertEqual(peer.id, smokeDeviceId)
        XCTAssertEqual(peer.name, smokeDeviceName)
        XCTAssertEqual(peer.status, .connected)
        XCTAssertFalse(try XCTUnwrap(link.waitForBatch()).isEmpty)

        await session.stop()
    }

    func testTheSnapshotReportsTheHostAndTheLivePeer() async throws {
        let link = LoopbackLinkTransport()
        let sink = RecordingEventSink()
        let session = CompanionSession.create(config: config(scratch()), backends: backends(link), events: sink)

        try await session.start()
        _ = try XCTUnwrap(sink.waitFor { event -> SessionPeer? in
            guard case let .peerConnected(peer) = event else { return nil }
            return peer
        })

        let snapshot = await session.snapshot()
        XCTAssertEqual(snapshot.hostInfo.appName, "smoke")
        XCTAssertEqual(snapshot.peers.map(\.id), [smokeDeviceId])
        XCTAssertEqual(snapshot.peers.first?.status, .connected)

        await session.stop()
    }

    func testLogInboxFansOutToTheEventSink() throws {
        let sink = RecordingEventSink()
        let session = CompanionSession.create(
            config: config(scratch()), backends: backends(LoopbackLinkTransport()), events: sink
        )

        session.logInbox().push(level: .warn, target: "platform", message: "hello from swift")

        let line = try XCTUnwrap(sink.waitFor { event -> (LogOrigin, LogLevel, String, String, UInt64)? in
            guard case let .log(origin, level, target, message, tsUnixMs) = event else { return nil }
            return (origin, level, target, message, tsUnixMs)
        })
        XCTAssertEqual(line.0, .host)
        XCTAssertEqual(line.1, .warn)
        XCTAssertEqual(line.2, "platform")
        XCTAssertEqual(line.3, "hello from swift")
        XCTAssertGreaterThan(line.4, 0, "the line carries the moment the ring stamped it")
    }
}
