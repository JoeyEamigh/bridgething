#if canImport(AVFoundation)

    import AVFoundation
    import BridgethingCompanionCore
    import Foundation

    enum ShellAudioSession {
        static func activateMixedPlayback() {
            #if os(iOS)
                let session = AVAudioSession.sharedInstance()
                try? session.setCategory(.playback, mode: .default, options: [.mixWithOthers])
                try? session.setActive(true)
            #endif
        }
    }

    public final class AvAudioBackend: AudioBackend, @unchecked Sendable {
        private let synth = AVSpeechSynthesizer()
        private let delegate = SpeechDelegate()
        private let earconBundle: Bundle
        private let playerStore = EarconPlayerStore()

        public init(earconBundle: Bundle = .main) {
            self.earconBundle = earconBundle
            synth.delegate = delegate
        }

        public func speak(id: String, text: String, voice: String?, sink: SpeakSink) {
            ShellAudioSession.activateMixedPlayback()
            let utterance = AVSpeechUtterance(string: text)
            if let voice {
                utterance.voice = AVSpeechSynthesisVoice(identifier: voice) ?? AVSpeechSynthesisVoice(language: voice)
            }
            delegate.register(
                utterance,
                deadlineNanos: Self.speakDeadline(for: text),
                onStart: { sink.onStart() },
                onFinish: { completed in sink.onFinished(ok: completed) }
            )
            synth.speak(utterance)
        }

        private static func speakDeadline(for text: String) -> UInt64 {
            let perChar = 0.12
            let seconds = max(15.0, Double(text.count) * perChar + 10.0)
            return UInt64(seconds * 1_000_000_000)
        }

        public func cancel(id: String) {
            synth.stopSpeaking(at: .immediate)
        }

        public func cancelAll() {
            synth.stopSpeaking(at: .immediate)
        }

        public func playEarcon(name: String, sink: EarconSink) {
            ShellAudioSession.activateMixedPlayback()
            let exts = ["wav", "caf", "aiff", "m4a", "mp3"]
            let bundle = earconBundle
            let url = exts.lazy
                .compactMap { bundle.url(forResource: name, withExtension: $0, subdirectory: "earcons") }
                .first
            guard let url, let player = try? AVAudioPlayer(contentsOf: url) else {
                sink.onFinished(ok: false)
                return
            }
            playerStore.retainWhilePlaying(player) { completed in sink.onFinished(ok: completed) }
            if !player.play() {
                playerStore.resolve(player, completed: false)
            }
        }
    }

    private final class SpeechDelegate: NSObject, AVSpeechSynthesizerDelegate, @unchecked Sendable {
        private struct Entry {
            let onStart: @Sendable () -> Void
            let onFinish: @Sendable (Bool) -> Void
        }

        private let lock = NSLock()
        private var entries: [ObjectIdentifier: Entry] = [:]
        private var watchdogs: [ObjectIdentifier: Task<Void, Never>] = [:]

        func register(
            _ utterance: AVSpeechUtterance,
            deadlineNanos: UInt64 = 0,
            onStart: @escaping @Sendable () -> Void,
            onFinish: @escaping @Sendable (Bool) -> Void
        ) {
            let key = ObjectIdentifier(utterance)
            lock.lock()
            entries[key] = Entry(onStart: onStart, onFinish: onFinish)
            if deadlineNanos > 0 {
                watchdogs[key] = Task { [weak self] in
                    try? await Task.sleep(nanoseconds: deadlineNanos)
                    self?.resolve(key, completed: false)
                }
            }
            lock.unlock()
        }

        func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didStart utterance: AVSpeechUtterance) {
            lock.lock()
            let entry = entries[ObjectIdentifier(utterance)]
            lock.unlock()
            entry?.onStart()
        }

        func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
            resolve(ObjectIdentifier(utterance), completed: true)
        }

        func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
            resolve(ObjectIdentifier(utterance), completed: false)
        }

        func resolve(_ key: ObjectIdentifier, completed: Bool) {
            lock.lock()
            let entry = entries.removeValue(forKey: key)
            watchdogs.removeValue(forKey: key)?.cancel()
            lock.unlock()
            entry?.onFinish(completed)
        }
    }

    private final class EarconPlayerStore: NSObject, AVAudioPlayerDelegate, @unchecked Sendable {
        private let lock = NSLock()
        private var players: [ObjectIdentifier: (AVAudioPlayer, @Sendable (Bool) -> Void)] = [:]

        func retainWhilePlaying(_ player: AVAudioPlayer, onFinish: @escaping @Sendable (Bool) -> Void) {
            player.delegate = self
            lock.lock()
            players[ObjectIdentifier(player)] = (player, onFinish)
            lock.unlock()
        }

        func resolve(_ player: AVAudioPlayer, completed: Bool) {
            lock.lock()
            let entry = players.removeValue(forKey: ObjectIdentifier(player))
            lock.unlock()
            entry?.1(completed)
        }

        func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
            resolve(player, completed: flag)
        }
    }

#endif
