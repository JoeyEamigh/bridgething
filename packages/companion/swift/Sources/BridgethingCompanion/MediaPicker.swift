#if os(iOS)

    import BridgethingCompanionCore
    import Foundation
    import Logging
    import ObjectiveC
    import PhotosUI
    import UIKit
    import UniformTypeIdentifiers

    private let pickerLog = Logger(label: "com.bridgething.companion.picker")

    @MainActor
    final class MediaPicker: NSObject, PHPickerViewControllerDelegate, UIDocumentPickerDelegate,
        UIAdaptivePresentationControllerDelegate
    {
        private static let slot = PickSlot()
        private static var ownerKey: UInt8 = 0

        private let staging: URL
        private let flight: PickFlight

        private init(staging: URL, flight: PickFlight) {
            self.staging = staging
            self.flight = flight
        }

        static func pick(kind: MediaPickKind, staging: URL) async throws -> PickedFile? {
            do {
                let picked = try await slot.pick { flight in
                    let picker = MediaPicker(staging: staging, flight: flight)
                    let controller = picker.controller(for: kind)
                    objc_setAssociatedObject(controller, &ownerKey, picker, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)
                    controller.presentationController?.delegate = picker
                    flight.present(controller, over: topController)
                }
                pickerLog.info("the media picker settled: \(String(describing: picked))")
                return picked
            } catch {
                pickerLog.info("the media picker failed: \(error)")
                throw error
            }
        }

        private func controller(for kind: MediaPickKind) -> UIViewController {
            switch kind {
            case .image, .video:
                var configuration = PHPickerConfiguration()
                configuration.filter = kind == .image ? .images : .videos
                configuration.selectionLimit = 1
                configuration.preferredAssetRepresentationMode = .current
                let picker = PHPickerViewController(configuration: configuration)
                picker.delegate = self
                return picker
            case .any:
                let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.item], asCopy: true)
                picker.allowsMultipleSelection = false
                picker.delegate = self
                return picker
            }
        }

        func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
            picker.dismiss(animated: true)
            guard let provider = results.first?.itemProvider else {
                flight.settle(.success(nil))
                return
            }
            let wanted = provider.registeredTypeIdentifiers.first { identifier in
                guard let type = UTType(identifier) else { return false }
                return type.conforms(to: .image) || type.conforms(to: .movie)
            }
            guard let identifier = wanted else {
                flight.settle(.failure(MediaPickerError.unreadable("no image or movie representation")))
                return
            }
            let staging = staging
            let flight = flight
            provider.loadFileRepresentation(forTypeIdentifier: identifier) { @Sendable url, error in
                let staged = Result<PickedFile?, Error> {
                    guard let url else {
                        throw MediaPickerError.unreadable(error?.localizedDescription ?? "nothing was delivered")
                    }
                    return try Self.stage(url, into: staging, mime: UTType(identifier)?.preferredMIMEType)
                }
                Task { @MainActor in flight.settle(staged) }
            }
        }

        func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
            flight.settle(
                Result {
                    guard let url = urls.first else { return nil }
                    let mime = (try? url.resourceValues(forKeys: [.contentTypeKey]))?.contentType?.preferredMIMEType
                    let picked = try Self.stage(url, into: staging, mime: mime)
                    try? FileManager.default.removeItem(at: url)
                    return picked
                }
            )
        }

        func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
            flight.settle(.success(nil))
        }

        func presentationControllerDidDismiss(_ presentationController: UIPresentationController) {
            flight.settle(.success(nil))
        }

        private nonisolated static func stage(_ url: URL, into staging: URL, mime: String?) throws -> PickedFile {
            try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
            let target = staging.appendingPathComponent(UUID().uuidString)
                .appendingPathExtension(url.pathExtension)
            try FileManager.default.copyItem(at: url, to: target)
            return PickedFile(url: target, mime: mime)
        }

        private static func topController() -> UIViewController? {
            let root = UIApplication.shared.connectedScenes
                .compactMap { $0 as? UIWindowScene }
                .flatMap(\.windows)
                .first(where: \.isKeyWindow)?
                .rootViewController
            var top = root
            while let presented = top?.presentedViewController {
                top = presented
            }
            return top
        }
    }

    struct CoordinatedTransition: PickTransition {
        let owner: String
        let coordinator: any UIViewControllerTransitionCoordinator

        func afterwards(_ then: @escaping @MainActor () -> Void) -> Bool {
            let queued = coordinator.animate(alongsideTransition: nil) { _ in DispatchQueue.main.async { then() } }
            if queued {
                pickerLog.info("waiting for \(owner) to finish its transition before presenting the media picker")
            }
            return queued
        }
    }

    extension UIViewController: PickPresenter {
        var ongoing: CoordinatedTransition? {
            transitionCoordinator.map { CoordinatedTransition(owner: "\(type(of: self))", coordinator: $0) }
        }

        func show(_ sheet: UIViewController) {
            pickerLog.info("presenting the media picker over \(type(of: self))")
            present(sheet, animated: true)
        }
    }

    extension BridgethingCompanion {
        public func pickMedia(kind: MediaPickKind, cover: MediaCover?) async throws -> PickedMedia? {
            let staging = FileManager.default.temporaryDirectory
                .appendingPathComponent("bridgething-pick", isDirectory: true)
            guard let picked = try await MediaPicker.pick(kind: kind, staging: staging) else { return nil }
            defer { try? FileManager.default.removeItem(at: picked.url) }
            guard !Task.isCancelled else { return nil }
            let session = session
            let ticket = session.mediaImportTicket()
            do {
                return try await withTaskCancellationHandler {
                    try await session.importPickedMedia(
                        ticket: ticket, path: picked.url.path, kind: kind, cover: cover, mimeHint: picked.mime
                    )
                } onCancel: {
                    session.cancelMediaImport(ticket: ticket)
                }
            } catch CompanionError.Cancelled where Task.isCancelled {
                return nil
            }
        }
    }

#endif
