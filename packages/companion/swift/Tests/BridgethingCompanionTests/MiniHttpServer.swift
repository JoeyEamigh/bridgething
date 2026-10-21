import Foundation

#if canImport(Glibc)
    import Glibc
#endif
#if canImport(Darwin)
    import Darwin
#endif

#if canImport(Glibc)
    private let sockStream = Int32(SOCK_STREAM.rawValue)
    private let shutReadWrite = Int32(SHUT_RDWR)
#else
    private let sockStream = SOCK_STREAM
    private let shutReadWrite = SHUT_RDWR
#endif

enum WebsocketPeer {
    case silent
    case answersPings
}

final class MiniHttpServer: @unchecked Sendable {
    typealias Response = (status: Int, headers: [(String, String)], body: Data)
    typealias Handler = @Sendable (_ method: String, _ path: String, _ body: Data) -> Response

    let port: UInt16
    private let listenFd: Int32
    private let handler: Handler

    init?(websocketPeer: WebsocketPeer? = nil, handler: @escaping Handler) {
        self.handler = handler
        let fd = socket(AF_INET, sockStream, 0)
        guard fd >= 0 else { return nil }
        var yes: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &yes, socklen_t(MemoryLayout<Int32>.size))

        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = UInt16(0).bigEndian
        addr.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))
        let bound = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bound == 0, listen(fd, 16) == 0 else {
            close(fd)
            return nil
        }
        var bound2 = sockaddr_in()
        var len = socklen_t(MemoryLayout<sockaddr_in>.size)
        _ = withUnsafeMutablePointer(to: &bound2) { ptr -> Int32 in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                getsockname(fd, $0, &len)
            }
        }
        listenFd = fd
        port = UInt16(bigEndian: bound2.sin_port)

        let serve = Thread { [handler] in
            while true {
                let client = accept(fd, nil, nil)
                if client < 0 { break }
                MiniHttpServer.serve(client: client, websocketPeer: websocketPeer, handler: handler)
            }
        }
        serve.name = "mini-http-server"
        serve.start()
    }

    func stop() {
        shutdown(listenFd, shutReadWrite)
        close(listenFd)
    }

    static func unusedPort() -> UInt16 {
        let fd = socket(AF_INET, sockStream, 0)
        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = UInt16(0).bigEndian
        addr.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))
        _ = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        var out = sockaddr_in()
        var len = socklen_t(MemoryLayout<sockaddr_in>.size)
        _ = withUnsafeMutablePointer(to: &out) { ptr -> Int32 in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                getsockname(fd, $0, &len)
            }
        }
        close(fd)
        return UInt16(bigEndian: out.sin_port)
    }

    private static func serve(client: Int32, websocketPeer: WebsocketPeer?, handler: Handler) {
        defer { close(client) }
        var buffer = Data()
        var scratch = [UInt8](repeating: 0, count: 16 * 1024)
        var headerEnd: Int? = nil
        while headerEnd == nil {
            let n = recv(client, &scratch, scratch.count, 0)
            if n <= 0 { return }
            buffer.append(contentsOf: scratch[0 ..< n])
            headerEnd = buffer.range(of: Data("\r\n\r\n".utf8))?.upperBound
            if buffer.count > 1 << 20 { return }
        }
        guard let headerEnd else { return }
        guard let head = String(data: buffer.prefix(headerEnd), encoding: .utf8) else { return }
        let lines = head.components(separatedBy: "\r\n")
        let request = lines.first?.components(separatedBy: " ") ?? []
        guard request.count >= 2 else { return }
        let method = request[0]
        let path = request[1]
        if let websocketPeer,
           let key = lines
           .first(where: { $0.lowercased().hasPrefix("sec-websocket-key:") })?
           .dropFirst("sec-websocket-key:".count)
           .trimmingCharacters(in: .whitespaces)
        {
            let accept = Data(sha1(Array((key + websocketGuid).utf8))).base64EncodedString()
            sendAll(
                client,
                Data(
                    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: \(accept)\r\n\r\n"
                        .utf8
                )
            )
            var frames = Data(buffer.suffix(from: headerEnd))
            while true {
                if websocketPeer == .answersPings, let (ping, used) = maskedPing(in: frames) {
                    frames.removeFirst(used)
                    var pong = Data([0x8A, UInt8(ping.count)])
                    pong.append(ping)
                    sendAll(client, pong)
                    continue
                }
                let n = recv(client, &scratch, scratch.count, 0)
                if n <= 0 { return }
                frames.append(contentsOf: scratch[0 ..< n])
            }
        }
        let contentLength = lines
            .first { $0.lowercased().hasPrefix("content-length:") }
            .flatMap { Int($0.dropFirst("content-length:".count).trimmingCharacters(in: .whitespaces)) } ?? 0
        var body = Data(buffer.suffix(from: headerEnd))
        while body.count < contentLength {
            let n = recv(client, &scratch, scratch.count, 0)
            if n <= 0 { break }
            body.append(contentsOf: scratch[0 ..< n])
        }

        let response = handler(method, path, body)
        var out = "HTTP/1.1 \(response.status) X\r\n"
        out += "Content-Length: \(response.body.count)\r\n"
        out += "Connection: close\r\n"
        for (name, value) in response.headers {
            out += "\(name): \(value)\r\n"
        }
        out += "\r\n"
        var payload = Data(out.utf8)
        payload.append(response.body)
        sendAll(client, payload)
    }

    private static func sendAll(_ client: Int32, _ payload: Data) {
        payload.withUnsafeBytes { raw in
            guard var base = raw.bindMemory(to: UInt8.self).baseAddress else { return }
            var remaining = payload.count
            while remaining > 0 {
                let n = send(client, base, remaining, 0)
                if n <= 0 { break }
                base += n
                remaining -= n
            }
        }
    }
}

private func maskedPing(in frames: Data) -> (Data, Int)? {
    let bytes = [UInt8](frames)
    guard bytes.count >= 6, bytes[0] & 0x0F == 0x9 else { return nil }
    let length = Int(bytes[1] & 0x7F)
    guard length < 126, bytes.count >= 6 + length else { return nil }
    let mask = bytes[2 ..< 6]
    let payload = (0 ..< length).map { bytes[6 + $0] ^ mask[mask.startIndex + $0 % 4] }
    return (Data(payload), 6 + length)
}

private let websocketGuid = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

private func sha1(_ message: [UInt8]) -> [UInt8] {
    var h: [UInt32] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0]
    var padded = message
    padded.append(0x80)
    while padded.count % 64 != 56 {
        padded.append(0)
    }
    let bits = UInt64(message.count) * 8
    for shift in stride(from: 56, through: 0, by: -8) {
        padded.append(UInt8(truncatingIfNeeded: bits >> UInt64(shift)))
    }
    for chunk in stride(from: 0, to: padded.count, by: 64) {
        var w = [UInt32](repeating: 0, count: 80)
        for i in 0 ..< 16 {
            let at = chunk + i * 4
            w[i] = UInt32(padded[at]) << 24 | UInt32(padded[at + 1]) << 16 | UInt32(padded[at + 2]) << 8
                | UInt32(padded[at + 3])
        }
        for i in 16 ..< 80 {
            let mixed = w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]
            w[i] = mixed << 1 | mixed >> 31
        }
        var (a, b, c, d, e) = (h[0], h[1], h[2], h[3], h[4])
        for i in 0 ..< 80 {
            let (f, k): (UInt32, UInt32) = switch i {
            case 0 ..< 20: ((b & c) | (~b & d), 0x5A82_7999)
            case 20 ..< 40: (b ^ c ^ d, 0x6ED9_EBA1)
            case 40 ..< 60: ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC)
            default: (b ^ c ^ d, 0xCA62_C1D6)
            }
            let next = (a << 5 | a >> 27) &+ f &+ e &+ k &+ w[i]
            e = d
            d = c
            c = b << 30 | b >> 2
            b = a
            a = next
        }
        h[0] = h[0] &+ a
        h[1] = h[1] &+ b
        h[2] = h[2] &+ c
        h[3] = h[3] &+ d
        h[4] = h[4] &+ e
    }
    return h.flatMap { word in (0 ..< 4).map { UInt8(truncatingIfNeeded: word >> UInt32(24 - $0 * 8)) } }
}
