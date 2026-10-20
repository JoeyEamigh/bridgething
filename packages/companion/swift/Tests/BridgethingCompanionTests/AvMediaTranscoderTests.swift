import AVFoundation
import BridgethingCompanionCore
import CoreMedia
import CoreVideo
import Foundation
import ImageIO
import UniformTypeIdentifiers
import XCTest

@testable import BridgethingCompanion

private final class RecordingSink: TranscodeSink, @unchecked Sendable {
    private let lock = NSLock()
    private var outcomes: [Result<TranscodedVideo, TranscodeError>] = []
    let settled = XCTestExpectation(description: "transcode settled")

    struct TranscodeError: Error, Equatable {
        let reason: String
    }

    init() {
        super.init(noHandle: NoHandle())
    }

    required init(unsafeFromHandle handle: UInt64) {
        fatalError("unreachable")
    }

    var results: [Result<TranscodedVideo, TranscodeError>] {
        lock.withLock { outcomes }
    }

    override func complete(video: TranscodedVideo) {
        lock.withLock { outcomes.append(.success(video)) }
        settled.fulfill()
    }

    override func fail(reason: String) {
        lock.withLock { outcomes.append(.failure(TranscodeError(reason: reason))) }
        settled.fulfill()
    }
}

private enum Clip {
    static let width = 1920
    static let height = 1080
    static let quarterTurn = CGAffineTransform(a: 0, b: 1, c: -1, d: 0, tx: CGFloat(height), ty: 0)

    static func write(
        to url: URL,
        fps: Int32,
        transform: CGAffineTransform,
        seconds: Double = 1.5,
        grain: UInt32 = 0,
        size: (width: Int, height: Int) = (width, height)
    ) async throws {
        let frames = Int(Double(fps) * seconds)
        let writer = try AVAssetWriter(outputURL: url, fileType: .mov)
        let video = AVAssetWriterInput(mediaType: .video, outputSettings: [
            AVVideoCodecKey: AVVideoCodecType.h264,
            AVVideoWidthKey: size.width,
            AVVideoHeightKey: size.height,
        ])
        video.transform = transform
        video.expectsMediaDataInRealTime = false
        let adaptor = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: video, sourcePixelBufferAttributes: [
            kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey as String: size.width,
            kCVPixelBufferHeightKey as String: size.height,
        ])
        let audio = AVAssetWriterInput(mediaType: .audio, outputSettings: [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 44100,
            AVNumberOfChannelsKey: 1,
        ])
        audio.expectsMediaDataInRealTime = false
        writer.add(video)
        writer.add(audio)
        guard writer.startWriting() else { throw writer.error! }
        writer.startSession(atSourceTime: .zero)

        let rate = 44100
        let chunk = 1024
        let total = rate * frames / Int(fps)
        let format = try silenceFormat(rate: rate)
        var offset = 0
        var index = 0
        while index < frames || offset < total {
            var moved = false
            if index < frames, video.isReadyForMoreMediaData {
                let buffer = try frame(
                    pool: XCTUnwrap(adaptor.pixelBufferPool), size: size, tint: UInt8(index % 200), grain: grain, seed: index
                )
                XCTAssertTrue(adaptor.append(buffer, withPresentationTime: CMTime(value: CMTimeValue(index), timescale: fps)))
                index += 1
                if index == frames { video.markAsFinished() }
                moved = true
            }
            if offset < total, audio.isReadyForMoreMediaData {
                let count = min(chunk, total - offset)
                XCTAssertTrue(try audio.append(silence(format: format, at: offset, count: count, rate: rate)))
                offset += count
                if offset == total { audio.markAsFinished() }
                moved = true
            }
            if !moved {
                guard writer.status == .writing else { throw writer.error ?? CocoaError(.fileWriteUnknown) }
                try await Task.sleep(for: .milliseconds(2))
            }
        }

        await writer.finishWriting()
        XCTAssertEqual(writer.status, .completed, String(describing: writer.error))
    }

    private static func frame(
        pool: CVPixelBufferPool,
        size: (width: Int, height: Int),
        tint: UInt8,
        grain: UInt32,
        seed: Int
    ) throws -> CVPixelBuffer {
        var created: CVPixelBuffer?
        CVPixelBufferPoolCreatePixelBuffer(nil, pool, &created)
        let buffer = try XCTUnwrap(created)
        CVPixelBufferLockBaseAddress(buffer, [])
        defer { CVPixelBufferUnlockBaseAddress(buffer, []) }
        let base = try XCTUnwrap(CVPixelBufferGetBaseAddress(buffer)).assumingMemoryBound(to: UInt8.self)
        let stride = CVPixelBufferGetBytesPerRow(buffer)
        let shade = tint / 4
        let red: [UInt8] = [shade, shade, 255, 255]
        let blue: [UInt8] = [255, shade, shade, 255]
        let line = (0 ..< size.width).flatMap { $0 < size.width / 2 ? red : blue }
        var state = UInt32(truncatingIfNeeded: seed &* 2_654_435_761) | 1
        for row in 0 ..< size.height {
            line.withUnsafeBufferPointer { (base + row * stride).update(from: $0.baseAddress!, count: line.count) }
            guard grain > 0 else { continue }
            let pixels = base + row * stride
            for offset in 0 ..< size.width * 4 where offset % 4 != 3 {
                state ^= state << 13
                state ^= state >> 17
                state ^= state << 5
                pixels[offset] = pixels[offset] &+ UInt8(truncatingIfNeeded: state % grain)
            }
        }
        return buffer
    }

    private static func silenceFormat(rate: Int) throws -> CMAudioFormatDescription {
        var description = AudioStreamBasicDescription(
            mSampleRate: Float64(rate),
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kLinearPCMFormatFlagIsSignedInteger | kLinearPCMFormatFlagIsPacked,
            mBytesPerPacket: 2,
            mFramesPerPacket: 1,
            mBytesPerFrame: 2,
            mChannelsPerFrame: 1,
            mBitsPerChannel: 16,
            mReserved: 0
        )
        var format: CMAudioFormatDescription?
        CMAudioFormatDescriptionCreate(
            allocator: nil,
            asbd: &description,
            layoutSize: 0,
            layout: nil,
            magicCookieSize: 0,
            magicCookie: nil,
            extensions: nil,
            formatDescriptionOut: &format
        )
        return try XCTUnwrap(format)
    }

    private static func silence(format: CMAudioFormatDescription, at offset: Int, count: Int, rate: Int) throws
        -> CMSampleBuffer
    {
        var block: CMBlockBuffer?
        CMBlockBufferCreateWithMemoryBlock(
            allocator: nil,
            memoryBlock: nil,
            blockLength: count * 2,
            blockAllocator: nil,
            customBlockSource: nil,
            offsetToData: 0,
            dataLength: count * 2,
            flags: kCMBlockBufferAssureMemoryNowFlag,
            blockBufferOut: &block
        )
        let filled = try XCTUnwrap(block)
        CMBlockBufferFillDataBytes(with: 0, blockBuffer: filled, offsetIntoDestination: 0, dataLength: count * 2)
        var sample: CMSampleBuffer?
        CMAudioSampleBufferCreateReadyWithPacketDescriptions(
            allocator: nil,
            dataBuffer: filled,
            formatDescription: format,
            sampleCount: count,
            presentationTimeStamp: CMTime(value: CMTimeValue(offset), timescale: CMTimeScale(rate)),
            packetDescriptions: nil,
            sampleBufferOut: &sample
        )
        return try XCTUnwrap(sample)
    }
}

final class AvMediaTranscoderTests: XCTestCase {
    private var directory: URL!

    override func setUpWithError() throws {
        directory = FileManager.default.temporaryDirectory.appendingPathComponent("AvMediaTranscoderTests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: directory)
    }

    func testRotatedSixtyFpsClipWithAudioLandsAsUprightThirtyFpsH264Only() async throws {
        let source = directory.appendingPathComponent("source.mov")
        let destination = directory.appendingPathComponent("out.mp4")
        try await Clip.write(to: source, fps: 60, transform: Clip.quarterTurn)
        let sourceAsset = AVURLAsset(url: source)
        let sourceAudio = try await sourceAsset.loadTracks(withMediaType: .audio)
        XCTAssertEqual(sourceAudio.count, 1)
        let sourceDuration = try await sourceAsset.load(.duration).seconds
        let plan = planVideo(width: UInt32(Clip.height), height: UInt32(Clip.width), coverWidth: 800, coverHeight: 480, maxBitrate: nil)
        XCTAssertEqual(plan.width, 800)
        XCTAssertEqual(plan.height, 1422)

        let sink = RecordingSink()
        _ = AvMediaTranscoder().transcode(
            request: TranscodeRequest(
                source: source.path, destination: destination.path, coverWidth: 800, coverHeight: 480, maxBitrate: nil
            ),
            sink: sink
        )
        await fulfillment(of: [sink.settled], timeout: 60)

        XCTAssertEqual(sink.results.count, 1)
        let video = try sink.results[0].get()
        XCTAssertEqual(video.width, plan.width)
        XCTAssertEqual(video.height, plan.height)
        XCTAssertEqual(Double(video.durationMs) / 1000, sourceDuration, accuracy: 0.1)

        let output = AVURLAsset(url: destination)
        let tracks = try await output.load(.tracks)
        XCTAssertEqual(tracks.count, 1)
        let track = try XCTUnwrap(tracks.first)
        XCTAssertEqual(track.mediaType, .video)
        let (naturalSize, transform, frameRate, formats) = try await track.load(
            .naturalSize, .preferredTransform, .nominalFrameRate, .formatDescriptions
        )
        XCTAssertEqual(formats.map { CMFormatDescriptionGetMediaSubType($0) }, [kCMVideoCodecType_H264])
        XCTAssertEqual(naturalSize, CGSize(width: Int(plan.width), height: Int(plan.height)))
        XCTAssertTrue(transform.isIdentity)
        XCTAssertLessThanOrEqual(frameRate, 30.5)
        XCTAssertGreaterThanOrEqual(frameRate, 29)
        let outputDuration = try await output.load(.duration).seconds
        XCTAssertEqual(Double(video.durationMs) / 1000, outputDuration, accuracy: 0.01)

        let generator = AVAssetImageGenerator(asset: output)
        generator.appliesPreferredTrackTransform = true
        let (image, _) = try await generator.image(at: CMTime(value: 1, timescale: 2))
        XCTAssertEqual(image.width, Int(plan.width))
        XCTAssertEqual(image.height, Int(plan.height))
        let top = pixel(image, x: Int(plan.width) / 2, y: Int(plan.height) / 4)
        let bottom = pixel(image, x: Int(plan.width) / 2, y: Int(plan.height) * 3 / 4)
        XCTAssertGreaterThan(top.red, 180)
        XCTAssertLessThan(top.blue, 90)
        XCTAssertGreaterThan(bottom.blue, 180)
        XCTAssertLessThan(bottom.red, 90)
    }

    func testSlowerClipKeepsItsOwnFrameRate() async throws {
        let source = directory.appendingPathComponent("source.mov")
        let destination = directory.appendingPathComponent("out.mp4")
        try await Clip.write(to: source, fps: 24, transform: .identity)
        let plan = planVideo(width: UInt32(Clip.width), height: UInt32(Clip.height), coverWidth: 800, coverHeight: 480, maxBitrate: nil)
        XCTAssertEqual(plan.width, 854)
        XCTAssertEqual(plan.height, 480)

        let sink = RecordingSink()
        _ = AvMediaTranscoder().transcode(
            request: TranscodeRequest(
                source: source.path, destination: destination.path, coverWidth: 800, coverHeight: 480, maxBitrate: nil
            ),
            sink: sink
        )
        await fulfillment(of: [sink.settled], timeout: 60)

        let video = try XCTUnwrap(sink.results.first).get()
        XCTAssertEqual(video.width, plan.width)
        XCTAssertEqual(video.height, plan.height)
        let tracks = try await AVURLAsset(url: destination).loadTracks(withMediaType: .video)
        let track = try XCTUnwrap(tracks.first)
        let (naturalSize, frameRate) = try await track.load(.naturalSize, .nominalFrameRate)
        XCTAssertEqual(naturalSize, CGSize(width: Int(plan.width), height: Int(plan.height)))
        XCTAssertEqual(frameRate, 24, accuracy: 0.5)
    }

    func testMissingSourceFailsAndLeavesNoDestination() async throws {
        let destination = directory.appendingPathComponent("out.mp4")
        let sink = RecordingSink()

        _ = AvMediaTranscoder().transcode(
            request: TranscodeRequest(
                source: directory.appendingPathComponent("missing.mov").path,
                destination: destination.path,
                coverWidth: 800,
                coverHeight: 480,
                maxBitrate: nil
            ),
            sink: sink
        )
        await fulfillment(of: [sink.settled], timeout: 30)

        XCTAssertEqual(sink.results.count, 1)
        guard case let .failure(failure) = sink.results.first else {
            return XCTFail("expected a failure, got \(sink.results)")
        }
        XCTAssertFalse(failure.reason.isEmpty)
        XCTAssertFalse(FileManager.default.fileExists(atPath: destination.path))
    }

    func testPosterIsTheUprightFirstFrameAsAJpegTheScalerCanThumbnail() async throws {
        let source = directory.appendingPathComponent("source.mov")
        try await Clip.write(to: source, fps: 30, transform: Clip.quarterTurn)

        let poster = try XCTUnwrap(AvMediaTranscoder().poster(source: source.path))

        let decoded = try XCTUnwrap(CGImageSourceCreateWithData(poster as CFData, nil))
        XCTAssertEqual(CGImageSourceGetType(decoded) as String?, UTType.jpeg.identifier)
        let image = try XCTUnwrap(CGImageSourceCreateImageAtIndex(decoded, 0, nil))
        XCTAssertEqual(image.width, Clip.height)
        XCTAssertEqual(image.height, Clip.width)
        let top = pixel(image, x: image.width / 2, y: image.height / 4)
        let bottom = pixel(image, x: image.width / 2, y: image.height * 3 / 4)
        XCTAssertGreaterThan(top.red, 180)
        XCTAssertLessThan(top.blue, 90)
        XCTAssertGreaterThan(bottom.blue, 180)
        XCTAssertLessThan(bottom.red, 90)

        let thumbnail = try XCTUnwrap(
            ImageIoScaler().scale(
                bytes: poster,
                sniff: MediaSniff(format: .jpeg, kind: .image, animated: false, mime: "image/jpeg"),
                target: .thumbnail(maxEdge: 320, quality: 0.7)
            )
        )
        XCTAssertEqual(thumbnail.mime, "image/jpeg")
        XCTAssertEqual(max(thumbnail.width, thumbnail.height), 320)
    }

    func testCancellingAnInFlightTranscodeSettlesItAsAFailureAndLeavesNothing() async throws {
        let source = directory.appendingPathComponent("source.mov")
        let destination = directory.appendingPathComponent("out.mp4")
        try await Clip.write(to: source, fps: 30, transform: .identity, seconds: 6)

        let sink = RecordingSink()
        let job = AvMediaTranscoder().transcode(
            request: TranscodeRequest(
                source: source.path, destination: destination.path, coverWidth: 800, coverHeight: 480, maxBitrate: nil
            ),
            sink: sink
        )
        job.cancel()
        await fulfillment(of: [sink.settled], timeout: 30)

        XCTAssertEqual(sink.results.count, 1)
        guard case .failure = sink.results.first else {
            return XCTFail("a cancelled transcode must not complete, got \(sink.results)")
        }
        XCTAssertFalse(FileManager.default.fileExists(atPath: destination.path))
    }

    func testABitrateCeilingKeepsADetailedClipUnderIt() async throws {
        let source = directory.appendingPathComponent("grainy.mov")
        try await Clip.write(to: source, fps: 30, transform: .identity, seconds: 3, grain: 48, size: (960, 540))
        let ceiling: UInt32 = 300_000
        XCTAssertGreaterThan(
            planVideo(width: 960, height: 540, coverWidth: 800, coverHeight: 480, maxBitrate: nil).bitrate,
            ceiling * 3
        )

        var rates: [UInt32?: Float] = [:]
        for maxBitrate in [nil, ceiling] as [UInt32?] {
            let destination = directory.appendingPathComponent("out-\(maxBitrate ?? 0).mp4")
            let sink = RecordingSink()
            _ = AvMediaTranscoder().transcode(
                request: TranscodeRequest(
                    source: source.path,
                    destination: destination.path,
                    coverWidth: 800,
                    coverHeight: 480,
                    maxBitrate: maxBitrate
                ),
                sink: sink
            )
            await fulfillment(of: [sink.settled], timeout: 120)
            _ = try XCTUnwrap(sink.results.first).get()
            let tracks = try await AVURLAsset(url: destination).loadTracks(withMediaType: .video)
            let track = try XCTUnwrap(tracks.first)
            rates[maxBitrate] = try await track.load(.estimatedDataRate)
        }

        let capped = try XCTUnwrap(rates[ceiling])
        let free = try XCTUnwrap(rates[nil])
        XCTAssertLessThanOrEqual(capped, Float(ceiling) * 1.25, "the encoder overshot its ceiling: \(rates)")
        XCTAssertLessThan(capped * 2, free, "the grain is detailed enough that the ceiling bound the encode: \(rates)")
    }

    func testPosterOfSomethingThatIsNotAVideoIsNil() throws {
        let missing = directory.appendingPathComponent("missing.mov")
        XCTAssertNil(AvMediaTranscoder().poster(source: missing.path))
        let text = directory.appendingPathComponent("notes.txt")
        try Data("plain words".utf8).write(to: text)
        XCTAssertNil(AvMediaTranscoder().poster(source: text.path))
    }

    private func pixel(_ image: CGImage, x: Int, y: Int) -> (red: UInt8, green: UInt8, blue: UInt8) {
        var bytes = [UInt8](repeating: 0, count: 4)
        let context = CGContext(
            data: &bytes,
            width: 1,
            height: 1,
            bitsPerComponent: 8,
            bytesPerRow: 4,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        )!
        context.draw(image, in: CGRect(x: -x, y: y - image.height + 1, width: image.width, height: image.height))
        return (bytes[0], bytes[1], bytes[2])
    }
}
