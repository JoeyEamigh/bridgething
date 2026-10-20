import Foundation
import XCTest

@testable import BridgethingCompanion

private struct Unsettled: Error {}

private struct Refused: Error {}

@MainActor
private final class FakeTransition: PickTransition {
    var queues = true
    fileprivate var waiting: [@MainActor () -> Void] = []

    func afterwards(_ then: @escaping @MainActor () -> Void) -> Bool {
        guard queues else { return false }
        waiting.append(then)
        return true
    }
}

@MainActor
private final class FakeScreen: PickPresenter {
    var ongoing: FakeTransition?
    var shown: [String] = []

    var transitioning: Bool {
        get { ongoing != nil }
        set { ongoing = newValue ? FakeTransition() : nil }
    }

    func show(_ sheet: String) {
        shown.append(sheet)
    }

    func finishTransition() {
        let ready = ongoing?.waiting ?? []
        ongoing = nil
        ready.forEach { $0() }
    }
}

private func wait(for expectation: XCTestExpectation) async throws {
    guard await XCTWaiter().fulfillment(of: [expectation], timeout: 5) == .completed else { throw Unsettled() }
}

@MainActor
private final class Pick {
    var flight: PickFlight?
    private let task: Task<PickedFile?, Error>

    init(on slot: PickSlot, present: @escaping @MainActor (PickFlight) -> Void) {
        task = Task { try await slot.pick(present) }
    }

    static func open(
        on slot: PickSlot, presenting present: @escaping @MainActor (PickFlight) -> Void = { _ in }
    ) async throws -> Pick {
        let shown = XCTestExpectation(description: "the picker is presented")
        let presented = Presented()
        let pick = Pick(on: slot) { flight in
            presented.flight = flight
            present(flight)
            shown.fulfill()
        }
        try await wait(for: shown)
        pick.flight = presented.flight
        presented.flight = nil
        return pick
    }

    func outcome() async throws -> PickedFile? {
        let settled = XCTestExpectation(description: "the pick settles")
        Task {
            _ = await task.result
            settled.fulfill()
        }
        try await wait(for: settled)
        return try await task.value
    }
}

@MainActor
private final class Presented {
    var flight: PickFlight?
}

@MainActor
private final class Stack {
    var top: FakeScreen?

    init(_ top: FakeScreen?) {
        self.top = top
    }
}

@MainActor
final class PickFlightTests: XCTestCase {
    private func stagedFile() throws -> PickedFile {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("\(UUID().uuidString).gif")
        try Data("picked".utf8).write(to: url)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return PickedFile(url: url, mime: "image/gif")
    }

    private func assertBusy(_ slot: PickSlot, file: StaticString = #filePath, line: UInt = #line) async {
        let second = Pick(on: slot) { _ in XCTFail("a second picker was presented", file: file, line: line) }
        do {
            _ = try await second.outcome()
            XCTFail("the second pick settled without an error", file: file, line: line)
        } catch {
            let description = (error as? LocalizedError)?.errorDescription
            XCTAssertEqual(description, "busy: a pick is already in flight", file: file, line: line)
        }
    }

    func testAPickSettlesWithTheChosenFile() async throws {
        let slot = PickSlot()
        let file = try stagedFile()
        let pick = try await Pick.open(on: slot)

        pick.flight?.settle(.success(file))

        let picked = try await pick.outcome()
        XCTAssertEqual(picked?.url, file.url)
        XCTAssertEqual(picked?.mime, "image/gif")
        XCTAssertTrue(FileManager.default.fileExists(atPath: file.url.path))
    }

    func testASecondPickWhileOneIsOpenIsBusyAndLeavesTheFirstIntact() async throws {
        let slot = PickSlot()
        let file = try stagedFile()
        let first = try await Pick.open(on: slot)

        await assertBusy(slot)
        await assertBusy(slot)

        first.flight?.settle(.success(file))
        let picked = try await first.outcome()
        XCTAssertEqual(picked?.url, file.url)
    }

    func testTheSlotFreesTheMomentAPickSettles() async throws {
        let slot = PickSlot()
        let first = try await Pick.open(on: slot)

        first.flight?.settle(.success(nil))
        let second = try await Pick.open(on: slot)

        XCTAssertFalse(first.flight === second.flight)
        second.flight?.settle(.success(nil))
        let cancelled = try await first.outcome()
        XCTAssertNil(cancelled)
        let alsoCancelled = try await second.outcome()
        XCTAssertNil(alsoCancelled)
    }

    func testAPickWhosePickerGoesAwayUnansweredSettlesAsCancelled() async throws {
        let slot = PickSlot()
        let pick = try await Pick.open(on: slot)

        pick.flight = nil

        let picked = try await pick.outcome()
        XCTAssertNil(picked)
        let next = try await Pick.open(on: slot)
        next.flight?.settle(.success(nil))
    }

    func testAPickerThatIsNeverKeptSettlesAsCancelled() async throws {
        let slot = PickSlot()
        let pick = Pick(on: slot) { _ in }

        let picked = try await pick.outcome()

        XCTAssertNil(picked)
    }

    private func assertNoPresenter(_ pick: Pick, file: StaticString = #filePath, line: UInt = #line) async {
        do {
            _ = try await pick.outcome()
            XCTFail("the pick settled without an error", file: file, line: line)
        } catch {
            guard case .noPresenter = error as? MediaPickerError else {
                return XCTFail("\(error)", file: file, line: line)
            }
        }
    }

    private func present(_ sheet: String, over stack: Stack, on slot: PickSlot) -> Pick {
        Pick(on: slot) { flight in flight.present(sheet, over: { stack.top }) }
    }

    private func open(_ sheet: String, over stack: Stack, on slot: PickSlot) async throws -> Pick {
        try await Pick.open(on: slot) { flight in flight.present(sheet, over: { stack.top }) }
    }

    func testAPickerTheScreenAcceptsStaysOpenUntilItIsAnswered() async throws {
        let slot = PickSlot()
        let file = try stagedFile()
        let screen = FakeScreen()

        let pick = try await open("photos", over: Stack(screen), on: slot)

        XCTAssertEqual(screen.shown, ["photos"])
        await assertBusy(slot)
        pick.flight?.settle(.success(file))
        let picked = try await pick.outcome()
        XCTAssertEqual(picked?.url, file.url)
    }

    func testAPickWithNoScreenFailsAndFreesTheSlot() async throws {
        let slot = PickSlot()

        await assertNoPresenter(present("photos", over: Stack(nil), on: slot))

        let next = try await Pick.open(on: slot)
        next.flight?.settle(.success(nil))
    }

    func testAPickStartedMidTransitionWaitsForItThenPresentsOverWhateverIsOnTop() async throws {
        let slot = PickSlot()
        let file = try stagedFile()
        let closing = FakeScreen()
        closing.transitioning = true
        let root = FakeScreen()
        let stack = Stack(closing)

        let pick = try await open("photos", over: stack, on: slot)

        XCTAssertEqual(closing.shown, [])
        XCTAssertEqual(root.shown, [])
        await assertBusy(slot)

        stack.top = root
        closing.finishTransition()

        XCTAssertEqual(closing.shown, [])
        XCTAssertEqual(root.shown, ["photos"])
        pick.flight?.settle(.success(file))
        let picked = try await pick.outcome()
        XCTAssertEqual(picked?.url, file.url)
    }

    func testATransitionThatWillNotQueueTheWaitPresentsAtOnce() async throws {
        let slot = PickSlot()
        let screen = FakeScreen()
        screen.transitioning = true
        screen.ongoing?.queues = false

        let pick = try await open("photos", over: Stack(screen), on: slot)

        XCTAssertEqual(screen.shown, ["photos"])
        await assertBusy(slot)
        pick.flight?.settle(.success(nil))
        let cancelled = try await pick.outcome()
        XCTAssertNil(cancelled)
    }

    func testAPickWaitsOutEveryTransitionOnTheWayToASettledScreen() async throws {
        let slot = PickSlot()
        let closing = FakeScreen()
        closing.transitioning = true
        let opening = FakeScreen()
        opening.transitioning = true
        let stack = Stack(closing)

        let pick = try await open("photos", over: stack, on: slot)
        stack.top = opening
        closing.finishTransition()

        XCTAssertEqual(opening.shown, [])
        await assertBusy(slot)

        opening.finishTransition()

        XCTAssertEqual(closing.shown, [])
        XCTAssertEqual(opening.shown, ["photos"])
        pick.flight?.settle(.success(nil))
        let cancelled = try await pick.outcome()
        XCTAssertNil(cancelled)
    }

    func testAPickWhoseScreenGoesAwayDuringATransitionFailsAndFreesTheSlot() async throws {
        let slot = PickSlot()
        let closing = FakeScreen()
        closing.transitioning = true
        let stack = Stack(closing)
        let pick = try await open("photos", over: stack, on: slot)

        stack.top = nil
        closing.finishTransition()

        await assertNoPresenter(pick)
        XCTAssertEqual(closing.shown, [])
        let next = try await Pick.open(on: slot)
        next.flight?.settle(.success(nil))
    }

    func testOnlyTheFirstSettleCountsAndALateFileIsRemoved() async throws {
        let slot = PickSlot()
        let late = try stagedFile()
        let pick = try await Pick.open(on: slot)

        pick.flight?.settle(.success(nil))
        pick.flight?.settle(.failure(Refused()))
        pick.flight?.settle(.success(late))

        let picked = try await pick.outcome()
        XCTAssertNil(picked)
        XCTAssertFalse(FileManager.default.fileExists(atPath: late.url.path))
    }

    func testALateSettleFromAnOlderPickLeavesTheNewerOneOpen() async throws {
        let slot = PickSlot()
        let file = try stagedFile()
        let older = try await Pick.open(on: slot)
        older.flight?.settle(.success(nil))
        let newer = try await Pick.open(on: slot)

        older.flight?.settle(.success(nil))
        older.flight = nil

        await assertBusy(slot)
        newer.flight?.settle(.success(file))
        let picked = try await newer.outcome()
        XCTAssertEqual(picked?.url, file.url)
    }

    func testPickErrorsLeadWithTheirToken() {
        XCTAssertEqual(MediaPickerError.busy.errorDescription, "busy: a pick is already in flight")
        XCTAssertEqual(
            MediaPickerError.noPresenter.errorDescription, "unsupported: there is no screen to present the picker on"
        )
        XCTAssertEqual(
            MediaPickerError.unreadable("gone").errorDescription, "io: the picked file could not be read: gone"
        )
    }
}
