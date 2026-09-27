import Foundation

#if os(iOS)
    import AVFoundation
    import os
    import UIKit

    private let keepAliveLog = Logger(subsystem: "com.bridgething.companion", category: "keepalive")

    actor BackgroundAudioKeepAlive {
        private var player: AVAudioPlayer?
        private var observers: [NSObjectProtocol] = []
        private var watchdog: Task<Void, Never>?
        private var active = false

        func activate() {
            guard !active else { return }
            active = true
            keepAliveLog.info("keepalive activate")
            registerObservers()
            startPlayer()
            startWatchdog()
        }

        func deactivate() {
            active = false
            keepAliveLog.info("keepalive deactivate")
            watchdog?.cancel()
            watchdog = nil
            for token in observers { NotificationCenter.default.removeObserver(token) }
            observers.removeAll()
            player?.stop()
            player = nil
            ShellAudioSession.shared.deactivate()
        }

        private func startPlayer() {
            ShellAudioSession.shared.activateMixedPlayback()
            do {
                let p = try AVAudioPlayer(data: Self.silence)
                p.numberOfLoops = -1
                p.volume = 1
                p.play()
                player = p
            } catch {
                player = nil
            }
        }

        private func reassert() {
            guard active else { return }
            ShellAudioSession.shared.activateMixedPlayback()
            if let p = player {
                if !p.isPlaying { p.play() }
            } else {
                startPlayer()
            }
            keepAliveLog.info("keepalive tick playing=\(self.player?.isPlaying ?? false, privacy: .public)")
        }

        private func rebuild() {
            guard active else { return }
            player?.stop()
            player = nil
            startPlayer()
        }

        private func startWatchdog() {
            watchdog?.cancel()
            watchdog = Task { [weak self] in
                while !Task.isCancelled {
                    try? await Task.sleep(nanoseconds: 10 * 1_000_000_000)
                    await self?.reassert()
                }
            }
        }

        private func registerObservers() {
            let nc = NotificationCenter.default
            let session = AVAudioSession.sharedInstance()
            observers.append(nc.addObserver(
                forName: AVAudioSession.interruptionNotification, object: session, queue: nil
            ) { [weak self] note in
                guard
                    let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                    AVAudioSession.InterruptionType(rawValue: raw) == .ended
                else { return }
                Task { await self?.reassert() }
            })
            observers.append(nc.addObserver(
                forName: AVAudioSession.mediaServicesWereResetNotification, object: nil, queue: nil
            ) { [weak self] _ in
                Task { await self?.rebuild() }
            })
            observers.append(nc.addObserver(
                forName: AVAudioSession.routeChangeNotification, object: session, queue: nil
            ) { [weak self] _ in
                Task { await self?.reassert() }
            })
            for name in [
                UIApplication.didEnterBackgroundNotification,
                UIApplication.willEnterForegroundNotification,
                UIApplication.didBecomeActiveNotification,
            ] {
                observers.append(nc.addObserver(forName: name, object: nil, queue: nil) { [weak self] _ in
                    Task { await self?.reassert() }
                })
            }
        }

        private static let silence: Data = {
            let sampleRate = 8000
            let dataBytes = sampleRate * 2
            func le32(_ v: UInt32) -> [UInt8] {
                [UInt8(v & 0xFF), UInt8((v >> 8) & 0xFF), UInt8((v >> 16) & 0xFF), UInt8((v >> 24) & 0xFF)]
            }
            func le16(_ v: UInt16) -> [UInt8] { [UInt8(v & 0xFF), UInt8((v >> 8) & 0xFF)] }
            var d = Data()
            d.append(contentsOf: Array("RIFF".utf8))
            d.append(contentsOf: le32(UInt32(36 + dataBytes)))
            d.append(contentsOf: Array("WAVE".utf8))
            d.append(contentsOf: Array("fmt ".utf8))
            d.append(contentsOf: le32(16))
            d.append(contentsOf: le16(1))
            d.append(contentsOf: le16(1))
            d.append(contentsOf: le32(UInt32(sampleRate)))
            d.append(contentsOf: le32(UInt32(sampleRate * 2)))
            d.append(contentsOf: le16(2))
            d.append(contentsOf: le16(16))
            d.append(contentsOf: Array("data".utf8))
            d.append(contentsOf: le32(UInt32(dataBytes)))
            d.append(Data(count: dataBytes))
            return d
        }()
    }
#endif
