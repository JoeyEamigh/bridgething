import Foundation

#if os(iOS)
    import AVFoundation
#endif

enum ShellAudioSessionPolicy: Equatable {
    case inactive
    case mixed
    case exclusive
}

protocol StreamDucker: AnyObject, Sendable {
    func duckForSpeech()
    func restoreAfterSpeech()
}

final class ShellAudioSession: @unchecked Sendable {
    static let shared = ShellAudioSession()

    private let apply: @Sendable (ShellAudioSessionPolicy) -> Void
    private let lock = NSLock()
    private var streamActive = false
    private var mixedRequested = false
    private var speechDepth = 0
    private weak var ducker: (any StreamDucker)?

    init(apply: @escaping @Sendable (ShellAudioSessionPolicy) -> Void = ShellAudioSession.applyToSystem) {
        self.apply = apply
    }

    func activateMixedPlayback() {
        apply(
            lock.withLock {
                mixedRequested = true
                return policy()
            })
    }

    func deactivate() {
        apply(
            lock.withLock {
                mixedRequested = false
                return policy()
            })
    }

    func streamDidStart() {
        let (next, held) = lock.withLock { () -> (ShellAudioSessionPolicy, (any StreamDucker)?) in
            streamActive = true
            return (policy(), speechDepth > 0 ? ducker : nil)
        }
        apply(next)
        held?.duckForSpeech()
    }

    func streamDidStop() {
        apply(
            lock.withLock {
                streamActive = false
                return policy()
            })
    }

    func beginSpeech() {
        let held = lock.withLock { () -> (any StreamDucker)? in
            speechDepth += 1
            return speechDepth == 1 && streamActive ? ducker : nil
        }
        held?.duckForSpeech()
    }

    func endSpeech() {
        let held = lock.withLock { () -> (any StreamDucker)? in
            guard speechDepth > 0 else { return nil }
            speechDepth -= 1
            return speechDepth == 0 && streamActive ? ducker : nil
        }
        held?.restoreAfterSpeech()
    }

    func setDucker(_ ducker: any StreamDucker) {
        lock.withLock { self.ducker = ducker }
    }

    func clearDucker(_ ducker: any StreamDucker) {
        lock.withLock {
            if self.ducker === ducker { self.ducker = nil }
        }
    }

    private func policy() -> ShellAudioSessionPolicy {
        if streamActive { return .exclusive }
        return mixedRequested ? .mixed : .inactive
    }

    private static let applyToSystem: @Sendable (ShellAudioSessionPolicy) -> Void = { policy in
        #if os(iOS)
            let session = AVAudioSession.sharedInstance()
            switch policy {
            case .inactive:
                try? session.setActive(false, options: .notifyOthersOnDeactivation)
            case .mixed:
                try? session.setCategory(.playback, mode: .default, options: [.mixWithOthers])
                try? session.setActive(true)
            case .exclusive:
                try? session.setCategory(.playback, mode: .default, options: [])
                try? session.setActive(true)
            }
        #endif
    }
}
