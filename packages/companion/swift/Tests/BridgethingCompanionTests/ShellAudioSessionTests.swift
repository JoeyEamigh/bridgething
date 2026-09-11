import Foundation
import XCTest

@testable import BridgethingCompanion

private final class RecordingApplier: @unchecked Sendable {
    private let lock = NSLock()
    private var applied: [ShellAudioSessionPolicy] = []

    var policies: [ShellAudioSessionPolicy] {
        lock.withLock { applied }
    }

    var last: ShellAudioSessionPolicy? {
        lock.withLock { applied.last }
    }

    func apply(_ policy: ShellAudioSessionPolicy) {
        lock.withLock { applied.append(policy) }
    }
}

private final class RecordingDucker: StreamDucker, @unchecked Sendable {
    private let lock = NSLock()
    private var levels: [Float] = []

    var volumes: [Float] {
        lock.withLock { levels }
    }

    func duckForSpeech() {
        lock.withLock { levels.append(0.2) }
    }

    func restoreAfterSpeech() {
        lock.withLock { levels.append(1.0) }
    }
}

final class ShellAudioSessionTests: XCTestCase {
    private func makeSession() -> (ShellAudioSession, RecordingApplier) {
        let applier = RecordingApplier()
        return (ShellAudioSession(apply: { applier.apply($0) }), applier)
    }

    func testKeepAliveDrivesMixedThenInactive() {
        let (session, applier) = makeSession()

        session.activateMixedPlayback()
        XCTAssertEqual(applier.last, .mixed)

        session.deactivate()
        XCTAssertEqual(applier.last, .inactive)
    }

    func testStreamTakesTheSessionExclusively() {
        let (session, applier) = makeSession()

        session.activateMixedPlayback()
        session.streamDidStart()

        XCTAssertEqual(applier.last, .exclusive)
    }

    func testSpeechWhileStreamingNeverFlipsBackToMixed() {
        let (session, applier) = makeSession()

        session.streamDidStart()
        session.activateMixedPlayback()
        session.beginSpeech()
        session.activateMixedPlayback()

        XCTAssertFalse(applier.policies.contains(.mixed))
        XCTAssertEqual(applier.last, .exclusive)
    }

    func testSpeechWhileStreamingDucksAndRestoresTheStream() {
        let (session, _) = makeSession()
        let ducker = RecordingDucker()
        session.setDucker(ducker)
        session.streamDidStart()

        session.beginSpeech()
        XCTAssertEqual(ducker.volumes, [0.2])

        session.endSpeech()
        XCTAssertEqual(ducker.volumes, [0.2, 1.0])
    }

    func testOverlappingSpeechRestoresOnlyOnceTheLastOneEnds() {
        let (session, _) = makeSession()
        let ducker = RecordingDucker()
        session.setDucker(ducker)
        session.streamDidStart()

        session.beginSpeech()
        session.beginSpeech()
        session.endSpeech()
        XCTAssertEqual(ducker.volumes, [0.2])

        session.endSpeech()
        XCTAssertEqual(ducker.volumes, [0.2, 1.0])
    }

    func testSpeechWithoutAStreamDoesNotDuck() {
        let (session, applier) = makeSession()
        let ducker = RecordingDucker()
        session.setDucker(ducker)

        session.activateMixedPlayback()
        session.beginSpeech()
        session.endSpeech()

        XCTAssertEqual(ducker.volumes, [])
        XCTAssertEqual(applier.last, .mixed)
    }

    func testStreamStartedMidSpeechDucksImmediately() {
        let (session, _) = makeSession()
        let ducker = RecordingDucker()
        session.setDucker(ducker)

        session.beginSpeech()
        session.streamDidStart()

        XCTAssertEqual(ducker.volumes, [0.2])
    }

    func testKeepAliveDeactivateDoesNotDropAnActiveStream() {
        let (session, applier) = makeSession()

        session.activateMixedPlayback()
        session.streamDidStart()
        session.deactivate()

        XCTAssertFalse(applier.policies.contains(.inactive))
        XCTAssertEqual(applier.last, .exclusive)
    }

    func testStreamStopHandsTheSessionBackToTheKeepAlive() {
        let (session, applier) = makeSession()

        session.activateMixedPlayback()
        session.streamDidStart()
        session.streamDidStop()

        XCTAssertEqual(applier.last, .mixed)
    }

    func testStreamStopDeactivatesWhenNothingElseWantsTheSession() {
        let (session, applier) = makeSession()

        session.activateMixedPlayback()
        session.streamDidStart()
        session.deactivate()
        session.streamDidStop()

        XCTAssertEqual(applier.last, .inactive)
    }

    func testClearDuckerOnlyDropsTheRegisteredOne() {
        let (session, _) = makeSession()
        let first = RecordingDucker()
        let second = RecordingDucker()
        session.setDucker(second)
        session.clearDucker(first)
        session.streamDidStart()

        session.beginSpeech()

        XCTAssertEqual(second.volumes, [0.2])
    }
}
