#if canImport(AVFoundation)

    import AVFoundation
    import BridgethingCompanionCore
    import Foundation

    #if os(iOS)
        import MediaPlayer
        import UIKit
    #endif

    public final class StreamPlayer: StreamBackend, StreamDucker, @unchecked Sendable {
        private static let duckedVolume: Float = 0.2

        private let queue = DispatchQueue(label: "com.bridgething.companion.stream")
        private let session = ShellAudioSession.shared

        private var player: AVPlayer?
        private var item: AVPlayerItem?
        private var sink: StreamSink?
        private var observations: [NSKeyValueObservation] = []
        private var tokens: [NSObjectProtocol] = []
        private var timeObserver: Any?
        private var metadataOutput: AVPlayerItemMetadataOutput?
        private var metadataRelay: StreamMetadataRelay?
        private var assetLoad: Task<Void, Never>?
        private var discovered = MetadataFields()
        private var published = StreamMetadata(title: nil, artist: nil, album: nil, artworkUrl: nil, artwork: nil)
        private var presentation: StreamPresentation?
        private var station: String?
        private var lastStatus: StreamStatus?
        private var terminal = false
        private var ducked = false
        private var seekable = false
        private var live = false
        private var generation: UInt64 = 0

        #if os(iOS)
            private let remote = StreamRemoteCommands()
        #endif

        public init() {}

        public func appBundle() -> String {
            Bundle.main.bundleIdentifier ?? "com.bridgething.gateway"
        }

        public func play(source: StreamSource, sink: StreamSink) {
            queue.async { self.start(source: source, sink: sink) }
        }

        public func present(presentation: StreamPresentation) {
            queue.async {
                self.presentation = presentation
                self.publishNowPlaying()
            }
        }

        public func pause() {
            queue.async { self.pauseNow() }
        }

        public func resume() {
            queue.async { self.resumeNow() }
        }

        public func seekTo(positionMs: UInt32) {
            queue.async { self.seekNow(positionMs) }
        }

        public func stop() {
            queue.async { self.finish(reporting: nil) }
        }

        public func duckForSpeech() {
            queue.async {
                self.ducked = true
                self.applyVolume()
            }
        }

        public func restoreAfterSpeech() {
            queue.async {
                self.ducked = false
                self.applyVolume()
            }
        }

        // MARK: - playback

        private func start(source: StreamSource, sink: StreamSink) {
            teardown()
            guard let mediaUrl = URL(string: source.url), mediaUrl.scheme != nil else {
                sink.onStatus(status: .failed(reason: "not a playable url: \(source.url)"))
                return
            }
            self.sink = sink
            live = source.live
            station = source.station.flatMap { candidate in
                let trimmed = candidate.trimmingCharacters(in: .whitespacesAndNewlines)
                return trimmed.isEmpty ? nil : trimmed
            }
            let generation = self.generation

            let asset = AVURLAsset(url: mediaUrl)
            let item = AVPlayerItem(asset: asset)
            let player = AVPlayer(playerItem: item)
            self.item = item
            self.player = player
            applyVolume()

            session.setDucker(self)
            session.streamDidStart()

            observe(item: item)
            observe(player: player)
            observeAudioSession()
            attachMetadata(to: item, generation: generation)
            loadAssetMetadata(from: asset, generation: generation)
            installRemoteCommands()

            publishMetadata()
            report(.buffering)
            player.play()
        }

        private func pauseNow() {
            guard let player else { return }
            player.pause()
            reportTimeControl(player.timeControlStatus)
        }

        private func resumeNow() {
            guard let player else { return }
            session.streamDidStart()
            player.play()
            reportTimeControl(player.timeControlStatus)
        }

        private func seekNow(_ positionMs: UInt32) {
            guard let player else { return }
            terminal = false
            let target = CMTime(value: CMTimeValue(positionMs), timescale: 1000)
            player.seek(to: target, toleranceBefore: .zero, toleranceAfter: .zero) { [weak self] _ in
                guard let self else { return }
                self.queue.async {
                    guard player === self.player else { return }
                    self.emitTiming()
                    self.reportTimeControl(player.timeControlStatus)
                }
            }
        }

        private func finish(reporting status: StreamStatus?) {
            if let status { report(status) }
            teardown()
            session.streamDidStop()
            session.clearDucker(self)
        }

        private func applyVolume() {
            player?.volume = ducked ? Self.duckedVolume : 1.0
        }

        private func teardown() {
            generation &+= 1
            assetLoad?.cancel()
            assetLoad = nil
            for observation in observations { observation.invalidate() }
            observations.removeAll()
            for token in tokens { NotificationCenter.default.removeObserver(token) }
            tokens.removeAll()
            if let timeObserver, let player { player.removeTimeObserver(timeObserver) }
            timeObserver = nil
            if let metadataOutput {
                item?.remove(metadataOutput)
                metadataOutput.setDelegate(nil, queue: nil)
            }
            metadataOutput = nil
            metadataRelay = nil
            player?.pause()
            player?.replaceCurrentItem(with: nil)
            player = nil
            item = nil
            sink = nil
            discovered = MetadataFields()
            published = StreamMetadata(title: nil, artist: nil, album: nil, artworkUrl: nil, artwork: nil)
            presentation = nil
            station = nil
            lastStatus = nil
            terminal = false
            ducked = false
            seekable = false
            live = false
            removeRemoteCommands()
            clearNowPlaying()
        }

        // MARK: - reporting

        private func report(_ status: StreamStatus) {
            guard let sink, !terminal, status != lastStatus else { return }
            lastStatus = status
            switch status {
            case .ended, .failed:
                terminal = true
            default:
                break
            }
            sink.onStatus(status: status)
            if terminal { session.streamDidStop() }
            publishNowPlaying()
        }

        private func reportTimeControl(_ status: AVPlayer.TimeControlStatus) {
            switch status {
            case .playing:
                report(.playing)
            case .waitingToPlayAtSpecifiedRate:
                report(.buffering)
            case .paused:
                report(.paused)
            @unknown default:
                break
            }
        }

        private func emitTiming() {
            guard let player, let item, let sink else { return }
            let durationMs = live ? nil : Self.finiteMillis(item.duration)
            let finite = durationMs != nil
            if finite != seekable {
                seekable = finite
                setSeekCommandEnabled(finite)
            }
            let timing = StreamTiming(
                positionMs: Self.millis(player.currentTime()) ?? 0,
                durationMs: durationMs,
                seekable: finite
            )
            sink.onTiming(timing: timing)
            publishNowPlaying()
        }

        private func publishMetadata() {
            let next = StreamMetadata(
                title: discovered.title ?? station,
                artist: discovered.artist,
                album: discovered.album,
                artworkUrl: nil,
                artwork: discovered.artwork
            )
            guard next != published else { return }
            published = next
            sink?.onMetadata(metadata: next)
            publishNowPlaying()
        }

        private static func millis(_ time: CMTime) -> UInt32? {
            guard time.isNumeric, time.seconds.isFinite else { return nil }
            let ms = (time.seconds * 1000).rounded()
            guard ms > 0 else { return 0 }
            return UInt32(min(ms, Double(UInt32.max)))
        }

        private static func finiteMillis(_ time: CMTime) -> UInt32? {
            guard let ms = millis(time), ms > 0 else { return nil }
            return ms
        }

        // MARK: - observation

        private func observe(item: AVPlayerItem) {
            observations.append(
                item.observe(\.status, options: [.new]) { [weak self] observed, _ in
                    guard let self, observed.status == .failed else { return }
                    let reason = observed.error?.localizedDescription ?? "playback failed"
                    self.queue.async {
                        guard observed === self.item else { return }
                        self.report(.failed(reason: reason))
                    }
                })

            let center = NotificationCenter.default
            tokens.append(
                center.addObserver(forName: .AVPlayerItemDidPlayToEndTime, object: item, queue: nil) {
                    [weak self] _ in
                    guard let self else { return }
                    self.queue.async {
                        guard item === self.item else { return }
                        self.emitTiming()
                        self.report(.ended)
                    }
                })
            tokens.append(
                center.addObserver(
                    forName: .AVPlayerItemFailedToPlayToEndTime, object: item, queue: nil
                ) { [weak self] note in
                    guard let self else { return }
                    let error = note.userInfo?[AVPlayerItemFailedToPlayToEndTimeErrorKey] as? Error
                    let reason = error?.localizedDescription ?? "stream ended early"
                    self.queue.async {
                        guard item === self.item else { return }
                        self.report(.failed(reason: reason))
                    }
                })
            tokens.append(
                center.addObserver(forName: .AVPlayerItemPlaybackStalled, object: item, queue: nil) {
                    [weak self] _ in
                    guard let self else { return }
                    self.queue.async {
                        guard item === self.item else { return }
                        self.report(.buffering)
                    }
                })
        }

        private func observe(player: AVPlayer) {
            observations.append(
                player.observe(\.timeControlStatus, options: [.new]) { [weak self] observed, _ in
                    guard let self else { return }
                    let status = observed.timeControlStatus
                    self.queue.async {
                        guard observed === self.player else { return }
                        self.reportTimeControl(status)
                    }
                })
            timeObserver = player.addPeriodicTimeObserver(
                forInterval: CMTime(seconds: 1, preferredTimescale: 1), queue: queue
            ) { [weak self] _ in
                guard let self, player === self.player else { return }
                self.emitTiming()
            }
        }

        // MARK: - metadata

        private struct MetadataFields: Sendable {
            var title: String?
            var artist: String?
            var album: String?
            var artwork: Data?
        }

        private func attachMetadata(to item: AVPlayerItem, generation: UInt64) {
            let output = AVPlayerItemMetadataOutput(identifiers: nil)
            let relay = StreamMetadataRelay { [weak self] groups in
                guard let self else { return }
                nonisolated(unsafe) let items = groups.flatMap(\.items)
                Task { await self.ingest(items, generation: generation) }
            }
            output.setDelegate(relay, queue: queue)
            item.add(output)
            metadataOutput = output
            metadataRelay = relay
        }

        private func ingest(_ items: [AVMetadataItem], generation: UInt64) async {
            var fields = MetadataFields()
            for entry in items {
                guard let identifier = entry.identifier else { continue }
                if identifier == .commonIdentifierArtwork {
                    fields.artwork = await Self.artwork(of: entry) ?? fields.artwork
                    continue
                }
                guard let value = try? await entry.load(.stringValue), !value.isEmpty else { continue }
                switch identifier {
                case .icyMetadataStreamTitle, .commonIdentifierTitle: fields.title = value
                case .commonIdentifierArtist: fields.artist = value
                case .commonIdentifierAlbumName: fields.album = value
                default: continue
                }
            }
            let resolved = fields
            queue.async {
                guard self.generation == generation else { return }
                self.merge(resolved, overwrite: true)
            }
        }

        private func loadAssetMetadata(from asset: AVURLAsset, generation: UInt64) {
            assetLoad = Task { [weak self] in
                guard let items = try? await asset.load(.commonMetadata), !items.isEmpty else { return }
                var fields = MetadataFields()
                for entry in items {
                    guard let key = entry.commonKey else { continue }
                    if key == .commonKeyArtwork {
                        fields.artwork = await Self.artwork(of: entry) ?? fields.artwork
                        continue
                    }
                    guard let value = try? await entry.load(.stringValue), !value.isEmpty else { continue }
                    switch key {
                    case .commonKeyTitle: fields.title = value
                    case .commonKeyArtist: fields.artist = value
                    case .commonKeyAlbumName: fields.album = value
                    default: continue
                    }
                }
                let resolved = fields
                guard let self, !Task.isCancelled else { return }
                self.queue.async {
                    guard self.generation == generation else { return }
                    self.merge(resolved, overwrite: false)
                }
            }
        }

        private static func artwork(of entry: AVMetadataItem) async -> Data? {
            guard let data = try? await entry.load(.dataValue), !data.isEmpty else { return nil }
            return data
        }

        private func merge(_ fields: MetadataFields, overwrite: Bool) {
            discovered.title = overwrite ? fields.title ?? discovered.title : discovered.title ?? fields.title
            discovered.artist = overwrite ? fields.artist ?? discovered.artist : discovered.artist ?? fields.artist
            discovered.album = overwrite ? fields.album ?? discovered.album : discovered.album ?? fields.album
            discovered.artwork = overwrite ? fields.artwork ?? discovered.artwork : discovered.artwork ?? fields.artwork
            publishMetadata()
        }

        // MARK: - audio session and remote control

        #if os(iOS)

            private func observeAudioSession() {
                let center = NotificationCenter.default
                let audio = AVAudioSession.sharedInstance()
                tokens.append(
                    center.addObserver(
                        forName: AVAudioSession.interruptionNotification, object: audio, queue: nil
                    ) { [weak self] note in
                        guard
                            let self,
                            let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                            let type = AVAudioSession.InterruptionType(rawValue: raw)
                        else { return }
                        let resume =
                            (note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt)
                            .map { AVAudioSession.InterruptionOptions(rawValue: $0).contains(.shouldResume) } ?? false
                        self.queue.async {
                            switch type {
                            case .began: self.pauseNow()
                            case .ended where resume: self.resumeNow()
                            default: break
                            }
                        }
                    })
                tokens.append(
                    center.addObserver(
                        forName: AVAudioSession.routeChangeNotification, object: audio, queue: nil
                    ) { [weak self] note in
                        guard
                            let self,
                            let raw = note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt,
                            AVAudioSession.RouteChangeReason(rawValue: raw) == .oldDeviceUnavailable
                        else { return }
                        self.queue.async { self.pauseNow() }
                    })
            }

            private func installRemoteCommands() {
                remote.install(
                    play: { [weak self] in self?.onQueue { $0.resumeNow() } },
                    pause: { [weak self] in self?.onQueue { $0.pauseNow() } },
                    toggle: { [weak self] in self?.onQueue { $0.togglePlayPause() } },
                    stop: { [weak self] in self?.onQueue { $0.finish(reporting: .ended) } }
                )
            }

            private func onQueue(_ body: @escaping @Sendable (StreamPlayer) -> Void) {
                queue.async { body(self) }
            }

            private func togglePlayPause() {
                guard let player else { return }
                if player.timeControlStatus == .paused {
                    resumeNow()
                } else {
                    pauseNow()
                }
            }

            private func removeRemoteCommands() {
                remote.clear()
            }

            private func setSeekCommandEnabled(_ enabled: Bool) {
                guard enabled else {
                    remote.setSeek(nil)
                    return
                }
                remote.setSeek { [weak self] seconds in
                    self?.onQueue { $0.seekNow(UInt32(max(0, seconds * 1000).rounded())) }
                }
            }

            private func publishNowPlaying() {
                guard let player, let item, let presentation, sink != nil else { return }
                let durationMs = live ? nil : Self.finiteMillis(item.duration)
                let snapshot = NowPlayingSnapshot(
                    title: presentation.title,
                    artist: presentation.artist,
                    album: presentation.album,
                    artwork: presentation.artwork,
                    durationSeconds: durationMs.map { Double($0) / 1000 },
                    elapsedSeconds: Double(Self.millis(player.currentTime()) ?? 0) / 1000,
                    rate: Double(player.rate),
                    isLive: live
                )
                DispatchQueue.main.async { snapshot.publish() }
            }

            private func clearNowPlaying() {
                DispatchQueue.main.async { MPNowPlayingInfoCenter.default().nowPlayingInfo = nil }
            }

        #else

            private func observeAudioSession() {}
            private func installRemoteCommands() {}
            private func removeRemoteCommands() {}
            private func setSeekCommandEnabled(_ enabled: Bool) {}
            private func publishNowPlaying() {}
            private func clearNowPlaying() {}

        #endif
    }

    private final class StreamMetadataRelay: NSObject, AVPlayerItemMetadataOutputPushDelegate, @unchecked Sendable {
        private let onGroups: @Sendable ([AVTimedMetadataGroup]) -> Void

        init(onGroups: @escaping @Sendable ([AVTimedMetadataGroup]) -> Void) {
            self.onGroups = onGroups
        }

        func metadataOutput(
            _ output: AVPlayerItemMetadataOutput,
            didOutputTimedMetadataGroups groups: [AVTimedMetadataGroup],
            from track: AVPlayerItemTrack?
        ) {
            onGroups(groups)
        }
    }

    #if os(iOS)

        private struct NowPlayingSnapshot: Sendable {
            let title: String
            let artist: String?
            let album: String?
            let artwork: Data?
            let durationSeconds: Double?
            let elapsedSeconds: Double
            let rate: Double
            let isLive: Bool

            func publish() {
                var info: [String: Any] = [:]
                info[MPMediaItemPropertyTitle] = title
                if let artist { info[MPMediaItemPropertyArtist] = artist }
                if let album { info[MPMediaItemPropertyAlbumTitle] = album }
                if let durationSeconds { info[MPMediaItemPropertyPlaybackDuration] = durationSeconds }
                info[MPNowPlayingInfoPropertyElapsedPlaybackTime] = elapsedSeconds
                info[MPNowPlayingInfoPropertyPlaybackRate] = rate
                info[MPNowPlayingInfoPropertyIsLiveStream] = isLive
                if let artwork, let image = UIImage(data: artwork) {
                    info[MPMediaItemPropertyArtwork] = MPMediaItemArtwork(boundsSize: image.size) { _ in image }
                }
                MPNowPlayingInfoCenter.default().nowPlayingInfo = info
            }
        }

        private final class StreamRemoteCommands: @unchecked Sendable {
            private var tokens: [(MPRemoteCommand, Any)] = []
            private var seekToken: (MPRemoteCommand, Any)?

            func install(
                play: @escaping @Sendable () -> Void,
                pause: @escaping @Sendable () -> Void,
                toggle: @escaping @Sendable () -> Void,
                stop: @escaping @Sendable () -> Void
            ) {
                DispatchQueue.main.async {
                    self.clearOnMain()
                    let center = MPRemoteCommandCenter.shared()
                    self.bind(center.playCommand, play)
                    self.bind(center.pauseCommand, pause)
                    self.bind(center.togglePlayPauseCommand, toggle)
                    self.bind(center.stopCommand, stop)
                }
            }

            func setSeek(_ handler: (@Sendable (Double) -> Void)?) {
                DispatchQueue.main.async {
                    let command = MPRemoteCommandCenter.shared().changePlaybackPositionCommand
                    if let seekToken = self.seekToken {
                        seekToken.0.removeTarget(seekToken.1)
                        self.seekToken = nil
                    }
                    guard let handler else {
                        command.isEnabled = false
                        return
                    }
                    command.isEnabled = true
                    let token = command.addTarget { event in
                        guard let event = event as? MPChangePlaybackPositionCommandEvent else {
                            return .commandFailed
                        }
                        handler(event.positionTime)
                        return .success
                    }
                    self.seekToken = (command, token)
                }
            }

            func clear() {
                DispatchQueue.main.async { self.clearOnMain() }
            }

            private func clearOnMain() {
                for (command, token) in tokens {
                    command.removeTarget(token)
                    command.isEnabled = false
                }
                tokens.removeAll()
                if let seekToken {
                    seekToken.0.removeTarget(seekToken.1)
                    seekToken.0.isEnabled = false
                    self.seekToken = nil
                }
            }

            private func bind(_ command: MPRemoteCommand, _ handler: @escaping @Sendable () -> Void) {
                command.isEnabled = true
                let token = command.addTarget { _ in
                    handler()
                    return .success
                }
                tokens.append((command, token))
            }
        }

    #endif

#endif
