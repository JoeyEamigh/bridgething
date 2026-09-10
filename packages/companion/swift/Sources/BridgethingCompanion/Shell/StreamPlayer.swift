#if canImport(AVFoundation)

    import AVFoundation
    import BridgethingCompanionCore
    import Foundation

    /// Plays raw http(s) media URLs (internet radio, podcast episodes) with AVPlayer.
    /// The Car Thing has no speaker, so a webapp that wants audible audio for a stream
    /// URL hands it here and the phone plays it.
    public final class StreamPlayer: StreamBackend, @unchecked Sendable {
        private let lock = NSLock()
        private var player: AVPlayer?
        private var sink: StreamSink?
        private var statusObservation: NSKeyValueObservation?
        private var endObserver: NSObjectProtocol?

        public init() {}

        public func play(url: String, sink: StreamSink) {
            stop()
            ShellAudioSession.activateMixedPlayback()
            guard let streamURL = URL(string: url) else {
                sink.onStopped(error: "invalid url")
                return
            }
            let item = AVPlayerItem(url: streamURL)
            let player = AVPlayer(playerItem: item)
            lock.withLock {
                self.player = player
                self.sink = sink
            }
            statusObservation = item.observe(\.status, options: [.new]) { [weak self] item, _ in
                guard let self else { return }
                switch item.status {
                case .readyToPlay:
                    self.lock.withLock { self.sink }?.onStarted()
                case .failed:
                    self.finish(error: item.error?.localizedDescription ?? "playback failed")
                case .unknown:
                    break
                @unknown default:
                    break
                }
            }
            endObserver = NotificationCenter.default.addObserver(
                forName: .AVPlayerItemDidPlayToEndTime, object: item, queue: nil
            ) { [weak self] _ in
                self?.finish(error: nil)
            }
            player.play()
        }

        public func pause() {
            lock.withLock { player }?.pause()
        }

        public func resume() {
            ShellAudioSession.activateMixedPlayback()
            lock.withLock { player }?.play()
        }

        public func stop() {
            finish(error: nil)
        }

        private func finish(error: String?) {
            let (player, sink): (AVPlayer?, StreamSink?) = lock.withLock {
                let player = self.player
                let sink = self.sink
                self.player = nil
                self.sink = nil
                return (player, sink)
            }
            statusObservation?.invalidate()
            statusObservation = nil
            if let endObserver {
                NotificationCenter.default.removeObserver(endObserver)
                self.endObserver = nil
            }
            player?.pause()
            player?.replaceCurrentItem(with: nil)
            sink?.onStopped(error: error)
        }
    }

#endif
