#if canImport(AVFoundation)

    import AVFoundation
    import BridgethingCompanionCore
    import CoreMedia
    import Foundation
    import ImageIO
    import UniformTypeIdentifiers

    private final class PosterFrame: @unchecked Sendable {
        var image: CGImage?
    }

    public final class AvMediaTranscoder: MediaTranscoder, @unchecked Sendable {
        public init() {}

        public func transcode(request: TranscodeRequest, sink: TranscodeSink) -> TranscodeJob {
            AvTranscodeJob(Task.detached(priority: .utility) {
                let destination = URL(fileURLWithPath: request.destination)
                do {
                    sink.complete(video: try await Self.run(request, to: destination))
                } catch {
                    try? FileManager.default.removeItem(at: destination)
                    sink.fail(reason: String(describing: error))
                }
            })
        }

        public func poster(source: String) -> Data? {
            let generator = AVAssetImageGenerator(asset: AVURLAsset(url: URL(fileURLWithPath: source)))
            generator.appliesPreferredTrackTransform = true
            generator.requestedTimeToleranceBefore = .zero
            generator.requestedTimeToleranceAfter = .positiveInfinity
            let frame = PosterFrame()
            let done = DispatchSemaphore(value: 0)
            generator.generateCGImageAsynchronously(for: .zero) { image, _, _ in
                frame.image = image
                done.signal()
            }
            done.wait()
            guard let image = frame.image else { return nil }
            let encoded = NSMutableData()
            guard let destination = CGImageDestinationCreateWithData(encoded, UTType.jpeg.identifier as CFString, 1, nil)
            else { return nil }
            CGImageDestinationAddImage(
                destination, image, [kCGImageDestinationLossyCompressionQuality: 0.9] as CFDictionary
            )
            return CGImageDestinationFinalize(destination) ? encoded as Data : nil
        }

        private static func run(_ request: TranscodeRequest, to destination: URL) async throws -> TranscodedVideo {
            let asset = AVURLAsset(url: URL(fileURLWithPath: request.source))
            guard let track = try await asset.loadTracks(withMediaType: .video).first else {
                throw TranscodeFailure("source has no video track")
            }
            let (naturalSize, transform, nominalFrameRate, timeRange) = try await track.load(
                .naturalSize, .preferredTransform, .nominalFrameRate, .timeRange
            )
            let display = CGRect(origin: .zero, size: naturalSize).applying(transform)
            let displayWidth = abs(display.width).rounded()
            let displayHeight = abs(display.height).rounded()
            guard displayWidth >= 1, displayHeight >= 1 else {
                throw TranscodeFailure("source video has no pixels")
            }
            let plan = planVideo(
                width: UInt32(displayWidth),
                height: UInt32(displayHeight),
                coverWidth: request.coverWidth,
                coverHeight: request.coverHeight,
                maxBitrate: request.maxBitrate
            )
            let renderWidth = CGFloat(plan.width)
            let renderHeight = CGFloat(plan.height)
            let scale = max(renderWidth / displayWidth, renderHeight / displayHeight)
            let placement = transform
                .concatenating(CGAffineTransform(translationX: -display.minX, y: -display.minY))
                .concatenating(CGAffineTransform(scaleX: scale, y: scale))
                .concatenating(CGAffineTransform(
                    translationX: (renderWidth - displayWidth * scale) / 2,
                    y: (renderHeight - displayHeight * scale) / 2
                ))
            let span = CMTimeRange(start: .zero, end: timeRange.end)

            let layer = AVMutableVideoCompositionLayerInstruction(assetTrack: track)
            layer.setTransform(placement, at: .zero)
            let instruction = AVMutableVideoCompositionInstruction()
            instruction.timeRange = span
            instruction.layerInstructions = [layer]

            let capped = nominalFrameRate > Float(plan.maxFps) || nominalFrameRate <= 0
            let frameRate = capped ? Double(plan.maxFps) : Double(nominalFrameRate)
            let composition = AVMutableVideoComposition()
            composition.renderSize = CGSize(width: renderWidth, height: renderHeight)
            composition.frameDuration = CMTime(seconds: 1 / frameRate, preferredTimescale: 600_000)
            if !capped {
                composition.sourceTrackIDForFrameTiming = track.trackID
            }
            composition.instructions = [instruction]
            composition.colorPrimaries = AVVideoColorPrimaries_ITU_R_709_2
            composition.colorTransferFunction = AVVideoTransferFunction_ITU_R_709_2
            composition.colorYCbCrMatrix = AVVideoYCbCrMatrix_ITU_R_709_2

            let reader = try AVAssetReader(asset: asset)
            reader.timeRange = span
            let output = AVAssetReaderVideoCompositionOutput(
                videoTracks: [track],
                videoSettings: [
                    kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                ]
            )
            output.videoComposition = composition
            output.alwaysCopiesSampleData = false
            guard reader.canAdd(output) else { throw TranscodeFailure("reader rejected the composition output") }
            reader.add(output)

            try? FileManager.default.removeItem(at: destination)
            let writer = try AVAssetWriter(outputURL: destination, fileType: .mp4)
            let input = AVAssetWriterInput(
                mediaType: .video,
                outputSettings: [
                    AVVideoCodecKey: AVVideoCodecType.h264,
                    AVVideoWidthKey: Int(plan.width),
                    AVVideoHeightKey: Int(plan.height),
                    AVVideoColorPropertiesKey: [
                        AVVideoColorPrimariesKey: AVVideoColorPrimaries_ITU_R_709_2,
                        AVVideoTransferFunctionKey: AVVideoTransferFunction_ITU_R_709_2,
                        AVVideoYCbCrMatrixKey: AVVideoYCbCrMatrix_ITU_R_709_2,
                    ],
                    AVVideoCompressionPropertiesKey: [
                        AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
                        AVVideoAverageBitRateKey: Int(plan.bitrate),
                        AVVideoMaxKeyFrameIntervalDurationKey: Double(plan.keyframeSeconds),
                        AVVideoExpectedSourceFrameRateKey: frameRate,
                    ],
                ]
            )
            input.expectsMediaDataInRealTime = false
            guard writer.canAdd(input) else { throw TranscodeFailure("writer rejected the h264 input") }
            writer.add(input)

            try await TranscodePump(reader: reader, output: output, writer: writer, input: input).run(from: span.start)

            let duration = try await AVURLAsset(url: destination).load(.duration)
            return TranscodedVideo(
                width: plan.width,
                height: plan.height,
                durationMs: UInt64(max(0, (duration.seconds * 1000).rounded()))
            )
        }
    }

    private final class AvTranscodeJob: TranscodeJob {
        private let task: Task<Void, Never>

        init(_ task: Task<Void, Never>) {
            self.task = task
        }

        func cancel() {
            task.cancel()
        }
    }

    private struct TranscodeFailure: Error, CustomStringConvertible {
        let description: String

        init(_ description: String) {
            self.description = description
        }
    }

    private final class TranscodePump: @unchecked Sendable {
        private let queue = DispatchQueue(label: "com.bridgething.companion.transcode")
        private let reader: AVAssetReader
        private let output: AVAssetReaderOutput
        private let writer: AVAssetWriter
        private let input: AVAssetWriterInput
        private var continuation: CheckedContinuation<Void, Error>?
        private var cancelled = false
        private var settled = false

        init(reader: AVAssetReader, output: AVAssetReaderOutput, writer: AVAssetWriter, input: AVAssetWriterInput) {
            self.reader = reader
            self.output = output
            self.writer = writer
            self.input = input
        }

        func run(from start: CMTime) async throws {
            try Task.checkCancellation()
            guard writer.startWriting() else {
                throw writer.error ?? TranscodeFailure("writer failed to start")
            }
            guard reader.startReading() else {
                writer.cancelWriting()
                throw reader.error ?? TranscodeFailure("reader failed to start")
            }
            writer.startSession(atSourceTime: start)
            do {
                try await drain()
            } catch {
                reader.cancelReading()
                writer.cancelWriting()
                throw error
            }
            await writer.finishWriting()
            guard writer.status == .completed else {
                throw writer.error ?? TranscodeFailure("writer did not complete")
            }
        }

        private func drain() async throws {
            try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                    queue.async { [self] in
                        self.continuation = continuation
                        guard !cancelled else { return settle(failure: CancellationError()) }
                        input.requestMediaDataWhenReady(on: queue) { [self] in
                            guard !settled else { return }
                            while input.isReadyForMoreMediaData {
                                guard let sample = output.copyNextSampleBuffer() else {
                                    input.markAsFinished()
                                    settle(failure: reader.status == .failed
                                        ? reader.error ?? TranscodeFailure("reader failed")
                                        : nil)
                                    return
                                }
                                guard input.append(sample) else {
                                    settle(failure: writer.error ?? TranscodeFailure("writer refused a frame"))
                                    return
                                }
                            }
                        }
                    }
                }
            } onCancel: {
                queue.async { [self] in
                    cancelled = true
                    settle(failure: CancellationError())
                }
            }
        }

        private func settle(failure: Error?) {
            guard !settled, let continuation else { return }
            settled = true
            if let failure {
                continuation.resume(throwing: failure)
            } else {
                continuation.resume()
            }
        }
    }

#endif
