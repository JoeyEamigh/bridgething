import Foundation
import IOBluetooth
import Network

private let port: NWEndpoint.Port = 8893
private let maxFrame = 16 * 1024 * 1024
setbuf(stdout, nil)

private struct FrameBuffer {
    private var bytes = Data()

    mutating func reset() { bytes.removeAll() }

    mutating func append(_ chunk: Data) throws -> [Data] {
        bytes.append(chunk)
        var frames: [Data] = []
        while bytes.count >= 16 {
            guard bytes.first == 0xde, bytes.dropFirst().first == 0xad else {
                throw NSError(domain: "relay", code: 2, userInfo: [NSLocalizedDescriptionKey: "Invalid BridgeThing frame header"])
            }
            let length = bytes.dropFirst(8).prefix(8).reduce(UInt64(0)) { ($0 << 8) | UInt64($1) }
            guard length <= maxFrame else {
                throw NSError(domain: "relay", code: 3, userInfo: [NSLocalizedDescriptionKey: "Oversized BridgeThing frame"])
            }
            let total = 16 + Int(length)
            guard bytes.count >= total else { break }
            frames.append(Data(bytes.prefix(total)))
            bytes = Data(bytes.dropFirst(total))
        }
        return frames
    }
}

private final class Relay: NSObject, IOBluetoothRFCOMMChannelDelegate {
    private let queue = DispatchQueue(label: "carthing.bluetooth.relay")
    private var listener: NWListener?
    private var client: NWConnection?
    private var channel: IOBluetoothRFCOMMChannel?
    private var incoming = FrameBuffer()
    private var address = ""
    private var clearedBaseband = false
    private var connectStarted = Date()

    func start(address: String) throws {
        self.address = address

        let webSocket = NWProtocolWebSocket.Options()
        webSocket.autoReplyPing = true
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: .ipv4(.loopback), port: port)
        parameters.defaultProtocolStack.applicationProtocols.insert(webSocket, at: 0)
        let listener = try NWListener(using: parameters)
        self.listener = listener
        listener.newConnectionHandler = { [weak self] connection in
            self?.queue.async { self?.accept(connection) }
        }
        listener.stateUpdateHandler = { state in
            if case .failed(let error) = state {
                fputs("WebSocket listener failed: \(error)\n", stderr)
                exit(1)
            }
        }
        listener.start(queue: queue)
        print("Relay ready. In BridgeThing Desktop, connect to ws://127.0.0.1:\(port)/")
    }

    private func openBluetooth(for connection: NWConnection) {
        guard client === connection else { return }
        guard let device = IOBluetoothDevice(addressString: address) else {
            fputs("Unknown Bluetooth address: \(address)\n", stderr)
            client?.cancel()
            return
        }
        var opened: IOBluetoothRFCOMMChannel?
        let status = device.openRFCOMMChannelSync(&opened, withChannelID: 1, delegate: self)
        guard status == kIOReturnSuccess, let opened else {
            opened?.close()
            if Date().timeIntervalSince(connectStarted) > 10 {
                fputs("Car Thing Bluetooth connection timed out\n", stderr)
                connection.cancel()
                return
            }
            if !clearedBaseband && device.isConnected() {
                clearedBaseband = true
                _ = device.closeConnection()
            }
            queue.asyncAfter(deadline: .now() + 0.5) { [weak self, weak connection] in
                guard let self, let connection else { return }
                self.openBluetooth(for: connection)
            }
            return
        }
        channel = opened
        print("Car Thing Bluetooth connected")
        receive(on: connection)
    }

    private func accept(_ connection: NWConnection) {
        client?.cancel()
        channel?.close()
        channel = nil
        incoming.reset()
        clearedBaseband = false
        connectStarted = Date()
        client = connection
        connection.stateUpdateHandler = { [weak self, weak connection] state in
            guard let self, let connection else { return }
            switch state {
            case .ready: self.openBluetooth(for: connection)
            case .failed, .cancelled:
                if self.client === connection {
                    self.client = nil
                    self.channel?.close()
                    self.channel = nil
                }
            default: break
            }
        }
        connection.start(queue: queue)
    }

    private func receive(on connection: NWConnection) {
        connection.receiveMessage { [weak self, weak connection] data, context, _, error in
            guard let self, let connection, self.client === connection else { return }
            if let data, !data.isEmpty {
                guard let metadata = context?.protocolMetadata(definition: NWProtocolWebSocket.definition)
                        as? NWProtocolWebSocket.Metadata, metadata.opcode == .binary else {
                    connection.cancel()
                    return
                }
                self.writeBluetooth(data)
            }
            if error == nil { self.receive(on: connection) }
            else { connection.cancel() }
        }
    }

    private func writeBluetooth(_ data: Data) {
        guard let channel else { return }
        let chunkSize = max(1, min(Int(channel.getMTU()), Int(UInt16.max)))
        var bytes = [UInt8](data)
        for offset in stride(from: 0, to: bytes.count, by: chunkSize) {
            let count = min(chunkSize, bytes.count - offset)
            let status = bytes.withUnsafeMutableBytes { raw in
                channel.writeSync(raw.baseAddress!.advanced(by: offset), length: UInt16(count))
            }
            if status != kIOReturnSuccess {
                fputs("RFCOMM write failed: \(status)\n", stderr)
                client?.cancel()
                break
            }
        }
    }

    func rfcommChannelData(_ rfcommChannel: IOBluetoothRFCOMMChannel!, data dataPointer: UnsafeMutableRawPointer!, length: Int) {
        let data = Data(bytes: dataPointer, count: length)
        queue.async { [weak self] in
            guard let self else { return }
            do {
                for frame in try self.incoming.append(data) {
                    if let connection = self.client {
                        let metadata = NWProtocolWebSocket.Metadata(opcode: .binary)
                        let context = NWConnection.ContentContext(identifier: "bridge", metadata: [metadata])
                        connection.send(content: frame, contentContext: context, isComplete: true, completion: .contentProcessed { error in
                            if let error { fputs("WebSocket send failed: \(error)\n", stderr) }
                        })
                    }
                }
            } catch {
                fputs("\(error)\n", stderr)
                self.incoming.reset()
                self.client?.cancel()
            }
        }
    }

    func rfcommChannelClosed(_ rfcommChannel: IOBluetoothRFCOMMChannel!) {
        queue.async { [weak self] in
            guard let self, self.channel === rfcommChannel else { return }
            fputs("Car Thing Bluetooth disconnected\n", stderr)
            self.client?.cancel()
            self.channel = nil
        }
    }
}

if CommandLine.arguments.contains("--self-test") {
    let frame = Data([0xde, 0xad, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 1, 2, 3])
    var buffer = FrameBuffer()
    let first = try! buffer.append(Data(frame.prefix(10)))
    let rest = try! buffer.append(Data(frame.dropFirst(10)) + frame)
    precondition(first.isEmpty && rest == [frame, frame])
    print("Frame buffering OK")
    exit(0)
}

let paired = (IOBluetoothDevice.pairedDevices() as? [IOBluetoothDevice] ?? [])
let carThings = Set(paired.filter { $0.name?.contains("Car Thing") == true }.compactMap(\.addressString))
if CommandLine.arguments.contains("--probe") { exit(carThings.count == 1 ? 0 : 1) }
if let parentFlag = CommandLine.arguments.firstIndex(of: "--parent-pid"),
   CommandLine.arguments.indices.contains(parentFlag + 1),
   let parent = Int32(CommandLine.arguments[parentFlag + 1]) {
    Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in
        if getppid() != parent { exit(0) }
    }
}
let address = carThings.count == 1 ? carThings.first : nil
guard let address else {
    print("Pair exactly one Car Thing with this Mac to enable Bluetooth")
    print("Paired devices:")
    for device in paired { print("  \(device.addressString ?? "?")  \(device.name ?? "?")") }
    exit(1)
}
do {
    let relay = Relay()
    try relay.start(address: address)
    RunLoop.main.run()
} catch {
    fputs("\(error)\n", stderr)
    exit(1)
}
