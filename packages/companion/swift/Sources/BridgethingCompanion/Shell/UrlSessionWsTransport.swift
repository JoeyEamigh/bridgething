import BridgethingCompanionCore
import Foundation

#if canImport(FoundationNetworking)
    import FoundationNetworking
#endif

private let wsAbnormalClosure: UInt16 = 1006

public final class UrlSessionWsTransport: NSObject, WsTransport, @unchecked Sendable {
    private final class Conn: @unchecked Sendable {
        let id: String
        let task: URLSessionWebSocketTask
        let inbox: WsInbox
        var reported = false
        var silenced = false
        var pings: UInt64 = 0
        var pongs: UInt64 = 0

        init(id: String, task: URLSessionWebSocketTask, inbox: WsInbox) {
            self.id = id
            self.task = task
            self.inbox = inbox
        }
    }

    private let lock = NSLock()
    private let pingQueue = DispatchQueue(label: "bridgething.ws.ping")
    private var session: URLSession!
    private var connsById: [String: Conn] = [:]
    private var connsByTask: [Int: Conn] = [:]

    private let pingInterval: TimeInterval

    public init(pingInterval: TimeInterval = 3) {
        self.pingInterval = pingInterval
        super.init()
        let cfg = URLSessionConfiguration.default
        cfg.timeoutIntervalForRequest = 60
        cfg.timeoutIntervalForResource = TimeInterval(Int32.max)
        #if canImport(Darwin)
            cfg.networkServiceType = .responsiveData
            cfg.shouldUseExtendedBackgroundIdleMode = true
        #endif
        session = URLSession(configuration: cfg, delegate: WsDelegate(owner: self), delegateQueue: nil)
    }

    public func connect(connect: WsConnect, inbox: WsInbox) {
        guard let url = URL(string: connect.url) else {
            inbox.onClosed(id: connect.id, code: nil, reason: "invalid url: \(connect.url)")
            return
        }
        var req = URLRequest(url: url)
        for header in connect.headers {
            req.setValue(header.value, forHTTPHeaderField: header.name)
        }
        if !connect.protocols.isEmpty {
            req.setValue(connect.protocols.joined(separator: ", "), forHTTPHeaderField: "Sec-WebSocket-Protocol")
        }
        let task = session.webSocketTask(with: req)
        let conn = Conn(id: connect.id, task: task, inbox: inbox)
        let replaced: Conn?
        lock.lock()
        replaced = connsById[connect.id]
        if let replaced {
            replaced.silenced = true
            connsByTask.removeValue(forKey: replaced.task.taskIdentifier)
        }
        connsById[connect.id] = conn
        connsByTask[task.taskIdentifier] = conn
        lock.unlock()
        replaced?.task.cancel(with: .goingAway, reason: nil)
        task.resume()
        receive(conn)
    }

    public func send(id: String, frame: WsFrame) {
        lock.lock()
        let conn = connsById[id]
        lock.unlock()
        guard let conn else { return }
        let message: URLSessionWebSocketTask.Message = switch frame {
        case let .text(text): .string(text)
        case let .binary(bytes): .data(bytes)
        }
        conn.task.send(message) { _ in }
    }

    public func disconnect(id: String, code: UInt16?, reason: String?) {
        lock.lock()
        let conn = connsById[id]
        lock.unlock()
        guard let conn else { return }
        let closeCode = URLSessionWebSocketTask.CloseCode(rawValue: Int(code ?? 1000)) ?? .normalClosure
        conn.task.cancel(with: closeCode, reason: reason.map { Data($0.utf8) })
        reportClosed(conn, code: code ?? 1000, reason: reason ?? "")
    }

    // MARK: - pump

    private func receive(_ conn: Conn) {
        conn.task.receive { [weak self] result in
            guard let self else { return }
            switch result {
            case let .success(message):
                switch message {
                case let .string(text): conn.inbox.onText(id: conn.id, text: text)
                case let .data(bytes): conn.inbox.onBinary(id: conn.id, bytes: bytes)
                @unknown default: break
                }
                self.receive(conn)
            case let .failure(error):
                self.reportClosed(conn, code: wsAbnormalClosure, reason: "read error: \(error.localizedDescription)")
            }
        }
    }

    fileprivate func handleOpen(taskIdentifier: Int, acceptedProtocol: String?) {
        lock.lock()
        let conn = connsByTask[taskIdentifier]
        lock.unlock()
        guard let conn, !conn.silenced else { return }
        conn.inbox.onOpen(id: conn.id, acceptedProtocol: acceptedProtocol)
        schedulePing(conn)
    }

    private func schedulePing(_ conn: Conn) {
        pingQueue.asyncAfter(deadline: .now() + pingInterval) { [weak self] in
            self?.ping(conn)
        }
    }

    private func ping(_ conn: Conn) {
        lock.lock()
        let live = connsById[conn.id] === conn && !conn.reported
        conn.pings += 1
        let sent = conn.pings
        lock.unlock()
        guard live else { return }
        conn.task.sendPing { [weak self] error in
            guard let self else { return }
            if let error {
                self.reportClosed(conn, code: wsAbnormalClosure, reason: "ping failed: \(error.localizedDescription)")
                return
            }
            self.lock.lock()
            conn.pongs = max(conn.pongs, sent)
            self.lock.unlock()
            self.schedulePing(conn)
        }
        pingQueue.asyncAfter(deadline: .now() + pingInterval) { [weak self] in
            guard let self else { return }
            self.lock.lock()
            let answered = conn.pongs >= sent
            self.lock.unlock()
            guard !answered else { return }
            conn.task.cancel(with: .goingAway, reason: nil)
            self.reportClosed(conn, code: wsAbnormalClosure, reason: "ping timeout")
        }
    }

    fileprivate func handleClose(taskIdentifier: Int, code: UInt16, reason: String) {
        lock.lock()
        let conn = connsByTask[taskIdentifier]
        lock.unlock()
        guard let conn else { return }
        reportClosed(conn, code: code, reason: reason)
    }

    private func reportClosed(_ conn: Conn, code: UInt16, reason: String) {
        lock.lock()
        let deliver = !conn.reported && !conn.silenced
        conn.reported = true
        if connsById[conn.id] === conn { connsById.removeValue(forKey: conn.id) }
        connsByTask.removeValue(forKey: conn.task.taskIdentifier)
        lock.unlock()
        if deliver {
            conn.inbox.onClosed(id: conn.id, code: code, reason: reason)
        }
    }
}

private final class WsDelegate: NSObject, URLSessionWebSocketDelegate, URLSessionTaskDelegate, @unchecked Sendable {
    private weak var owner: UrlSessionWsTransport?

    init(owner: UrlSessionWsTransport) {
        self.owner = owner
    }

    func urlSession(
        _ session: URLSession, webSocketTask: URLSessionWebSocketTask,
        didOpenWithProtocol protocol: String?
    ) {
        owner?.handleOpen(taskIdentifier: webSocketTask.taskIdentifier, acceptedProtocol: `protocol`)
    }

    func urlSession(
        _ session: URLSession, webSocketTask: URLSessionWebSocketTask,
        didCloseWith closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?
    ) {
        let text = reason.flatMap { String(data: $0, encoding: .utf8) } ?? "closed"
        owner?.handleClose(
            taskIdentifier: webSocketTask.taskIdentifier,
            code: UInt16(clamping: closeCode.rawValue),
            reason: text
        )
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        owner?.handleClose(
            taskIdentifier: task.taskIdentifier,
            code: wsAbnormalClosure,
            reason: "connect failed: \(error.map { $0.localizedDescription } ?? "connection ended")"
        )
    }
}
