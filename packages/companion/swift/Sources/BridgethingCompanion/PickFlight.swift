import Foundation

struct PickedFile: Sendable {
    let url: URL
    let mime: String?
}

enum MediaPickerError: Error, LocalizedError {
    case busy
    case noPresenter
    case unreadable(String)

    var errorDescription: String? {
        switch self {
        case .busy: "busy: a pick is already in flight"
        case .noPresenter: "unsupported: there is no screen to present the picker on"
        case let .unreadable(reason): "io: the picked file could not be read: \(reason)"
        }
    }
}

@MainActor
final class PickFlight {
    private var continuation: CheckedContinuation<PickedFile?, Error>?

    fileprivate init(_ continuation: CheckedContinuation<PickedFile?, Error>) {
        self.continuation = continuation
    }

    deinit {
        continuation?.resume(returning: nil)
    }

    fileprivate var isOpen: Bool {
        continuation != nil
    }

    func settle(_ result: Result<PickedFile?, Error>) {
        guard let continuation else {
            if case let .success(late?) = result {
                try? FileManager.default.removeItem(at: late.url)
            }
            return
        }
        self.continuation = nil
        continuation.resume(with: result)
    }
}

@MainActor
protocol PickTransition {
    func afterwards(_ then: @escaping @MainActor () -> Void) -> Bool
}

@MainActor
protocol PickPresenter {
    associatedtype Sheet
    associatedtype Ongoing: PickTransition

    var ongoing: Ongoing? { get }
    func show(_ sheet: Sheet)
}

extension PickFlight {
    func present<Presenter: PickPresenter>(
        _ sheet: Presenter.Sheet, over top: @escaping @MainActor () -> Presenter?
    ) {
        guard let presenter = top() else { return settle(.failure(MediaPickerError.noPresenter)) }
        if let ongoing = presenter.ongoing, ongoing.afterwards({ [self] in present(sheet, over: top) }) {
            return
        }
        presenter.show(sheet)
    }
}

@MainActor
final class PickSlot {
    private weak var flight: PickFlight?

    func pick(_ present: (PickFlight) -> Void) async throws -> PickedFile? {
        guard flight?.isOpen != true else { throw MediaPickerError.busy }
        return try await withCheckedThrowingContinuation { continuation in
            let flight = PickFlight(continuation)
            self.flight = flight
            present(flight)
        }
    }
}
