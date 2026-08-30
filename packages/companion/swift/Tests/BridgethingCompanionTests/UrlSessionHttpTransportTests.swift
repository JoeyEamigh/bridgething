import BridgethingCompanionCore
import BridgethingCompanion
import Foundation
import XCTest

private let waitTimeout: TimeInterval = 10

private final class RecordingHttpSink: HttpSink, @unchecked Sendable {
    private let condition = NSCondition()
    private var outcome: Result<HttpResponse, Error>?

    init() {
        super.init(noHandle: .init())
    }

    required init(unsafeFromHandle handle: UInt64) {
        fatalError("test sink only")
    }

    override func complete(response: HttpResponse) {
        resolve(.success(response))
    }

    override func fail(reason: String) {
        resolve(.failure(NSError(domain: "test", code: 1, userInfo: [NSLocalizedDescriptionKey: reason])))
    }

    private func resolve(_ result: Result<HttpResponse, Error>) {
        condition.lock()
        if outcome == nil { outcome = result }
        condition.broadcast()
        condition.unlock()
    }

    func wait() -> Result<HttpResponse, Error>? {
        let deadline = Date().addingTimeInterval(waitTimeout)
        condition.lock()
        defer { condition.unlock() }
        while outcome == nil {
            if !condition.wait(until: deadline) { break }
        }
        return outcome
    }
}

private final class RecordingDownloadSink: HttpDownloadSink, @unchecked Sendable {
    private let condition = NSCondition()
    private(set) var events: [String] = []
    private(set) var received = Data()

    init() {
        super.init(noHandle: .init())
    }

    required init(unsafeFromHandle handle: UInt64) {
        fatalError("test sink only")
    }

    override func onResponse(status: UInt16, headers: [HttpHeader], contentLength: UInt64?) {
        push("response:\(status):\(contentLength.map(String.init) ?? "?")")
    }

    override func onChunk(chunk: Data) {
        condition.lock()
        received.append(chunk)
        condition.unlock()
    }

    override func onFinished() {
        push("finished")
    }

    override func onFailed(reason: String) {
        push("failed:\(reason)")
    }

    private func push(_ event: String) {
        condition.lock()
        events.append(event)
        condition.broadcast()
        condition.unlock()
    }

    func waitForTerminal() -> [String] {
        let deadline = Date().addingTimeInterval(waitTimeout)
        condition.lock()
        defer { condition.unlock() }
        while !events.contains(where: { $0 == "finished" || $0.hasPrefix("failed:") }) {
            if !condition.wait(until: deadline) { break }
        }
        return events
    }
}

final class UrlSessionHttpTransportTests: XCTestCase {
    private func request(
        _ url: String, method: HttpMethod = .get, body: Data = Data(), timeoutMs: UInt32 = 5000
    ) -> HttpRequest {
        HttpRequest(method: method, url: url, headers: [], body: body, timeoutMs: timeoutMs)
    }

    func testExecuteRoundTripsStatusHeadersAndBody() throws {
        let server = try XCTUnwrap(MiniHttpServer { _, _, _ in
            (200, [("x-thing", "yes")], Data("hi there".utf8))
        })
        defer { server.stop() }

        let sink = RecordingHttpSink()
        UrlSessionHttpTransport().execute(request: request("http://127.0.0.1:\(server.port)/hello"), sink: sink)
        let response = try XCTUnwrap(sink.wait()).get()
        XCTAssertEqual(response.status, 200)
        XCTAssertEqual(String(data: response.body, encoding: .utf8), "hi there")
        XCTAssertEqual(response.headers.first { $0.name.lowercased() == "x-thing" }?.value, "yes")
    }

    func testExecuteSendsTheBodyAndFailsOnlyThroughTheSink() throws {
        let receivedBody = LockedBox<Data>()
        let server = try XCTUnwrap(MiniHttpServer { method, path, body in
            if method == "POST", path == "/echo" {
                receivedBody.set(body)
                return (200, [], Data("ok".utf8))
            }
            return (404, [], Data())
        })
        defer { server.stop() }

        let sink = RecordingHttpSink()
        UrlSessionHttpTransport().execute(
            request: request("http://127.0.0.1:\(server.port)/echo", method: .post, body: Data([4, 5, 6])),
            sink: sink
        )
        _ = try XCTUnwrap(sink.wait()).get()
        XCTAssertEqual(receivedBody.get(), Data([4, 5, 6]))
    }

    func testAVerbTheEnumDoesNotNameReachesTheServerVerbatim() throws {
        let seen = LockedBox<String>()
        let server = try XCTUnwrap(MiniHttpServer { method, _, _ in
            seen.set(method)
            return (207, [], Data("ok".utf8))
        })
        defer { server.stop() }

        let sink = RecordingHttpSink()
        UrlSessionHttpTransport().execute(
            request: request("http://127.0.0.1:\(server.port)/dav", method: .other(verb: "PROPFIND")),
            sink: sink
        )
        let response = try XCTUnwrap(sink.wait()).get()
        XCTAssertEqual(response.status, 207)
        XCTAssertEqual(seen.get(), "PROPFIND")
    }

    func testAConnectFailureLandsAsFailNeverAHang() throws {
        let port = MiniHttpServer.unusedPort()
        let sink = RecordingHttpSink()
        UrlSessionHttpTransport().execute(request: request("http://127.0.0.1:\(port)/nope"), sink: sink)
        let outcome = try XCTUnwrap(sink.wait())
        guard case .failure = outcome else {
            return XCTFail("expected a fail, got \(outcome)")
        }
    }

    func testAnInvalidUrlFailsImmediately() throws {
        let sink = RecordingHttpSink()
        UrlSessionHttpTransport().execute(request: request(""), sink: sink)
        let outcome = try XCTUnwrap(sink.wait())
        guard case .failure = outcome else {
            return XCTFail("expected a fail, got \(outcome)")
        }
    }

    func testDownloadStreamsChunksAndFinishes() throws {
        let payload = Data((0 ..< 256 * 1024).map { UInt8($0 % 251) })
        let server = try XCTUnwrap(MiniHttpServer { _, _, _ in
            (200, [], payload)
        })
        defer { server.stop() }

        let sink = RecordingDownloadSink()
        UrlSessionHttpTransport().download(request: request("http://127.0.0.1:\(server.port)/artifact"), sink: sink)
        let events = sink.waitForTerminal()
        XCTAssertEqual(events.first, "response:200:\(payload.count)")
        XCTAssertEqual(events.last, "finished")
        XCTAssertEqual(sink.received, payload)
    }

    func testDownloadConnectFailureLandsAsFailed() throws {
        let port = MiniHttpServer.unusedPort()
        let sink = RecordingDownloadSink()
        UrlSessionHttpTransport().download(request: request("http://127.0.0.1:\(port)/artifact"), sink: sink)
        let events = sink.waitForTerminal()
        XCTAssertTrue(events.contains { $0.hasPrefix("failed:") }, "expected a failed event, got \(events)")
    }
}

private final class LockedBox<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: T?

    func set(_ new: T) {
        lock.lock()
        value = new
        lock.unlock()
    }

    func get() -> T? {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}
