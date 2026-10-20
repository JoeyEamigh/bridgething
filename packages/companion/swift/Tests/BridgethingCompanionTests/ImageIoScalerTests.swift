import BridgethingCompanionCore
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers
import XCTest

@testable import BridgethingCompanion

private func sniffed(_ format: MediaFormat, _ mime: String, animated: Bool = false) -> MediaSniff {
    MediaSniff(format: format, kind: .image, animated: animated, mime: mime)
}

private let unknown = MediaSniff(format: .unknown, kind: .other, animated: false, mime: nil)

private struct Rgba: Equatable {
    let red: UInt8
    let green: UInt8
    let blue: UInt8
    let alpha: UInt8
}

private enum Fixture {
    static let red = CGColor(srgbRed: 1, green: 0, blue: 0, alpha: 1)
    static let blue = CGColor(srgbRed: 0, green: 0, blue: 1, alpha: 1)
    static let clear = CGColor(srgbRed: 0, green: 0, blue: 0, alpha: 0)

    static func image(width: Int, height: Int, left: CGColor, right: CGColor, alpha: Bool = false) -> CGImage {
        let context = CGContext(
            data: nil,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: (alpha ? CGImageAlphaInfo.premultipliedLast : .noneSkipLast).rawValue
        )!
        context.clear(CGRect(x: 0, y: 0, width: width, height: height))
        context.setFillColor(left)
        context.fill(CGRect(x: 0, y: 0, width: width / 2, height: height))
        context.setFillColor(right)
        context.fill(CGRect(x: width / 2, y: 0, width: width - width / 2, height: height))
        return context.makeImage()!
    }

    static func encode(
        _ frames: [CGImage],
        as type: UTType,
        frameProperties: [CFString: Any] = [:],
        containerProperties: [CFString: Any] = [:]
    ) -> Data? {
        let out = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(out, type.identifier as CFString, frames.count, nil)
        else { return nil }
        CGImageDestinationSetProperties(destination, containerProperties as CFDictionary)
        for frame in frames {
            CGImageDestinationAddImage(destination, frame, frameProperties as CFDictionary)
        }
        guard CGImageDestinationFinalize(destination) else { return nil }
        return out as Data
    }

    static func withPrimaryItem(_ item: UInt16, in heif: Data) -> Data? {
        guard let box = heif.range(of: Data("pitm".utf8)) else { return nil }
        var patched = heif
        let id = box.upperBound + 4
        patched[id] = UInt8(item >> 8)
        patched[id + 1] = UInt8(item & 0xFF)
        return patched
    }

    static func decode(_ data: Data) -> (type: String, image: CGImage, properties: [CFString: Any]) {
        let source = CGImageSourceCreateWithData(data as CFData, nil)!
        let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any] ?? [:]
        return (CGImageSourceGetType(source)! as String, CGImageSourceCreateImageAtIndex(source, 0, nil)!, properties)
    }

    static func pixel(_ image: CGImage, x: Int, y: Int) -> Rgba {
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
        return Rgba(red: bytes[0], green: bytes[1], blue: bytes[2], alpha: bytes[3])
    }
}

final class ImageIoScalerTests: XCTestCase {
    private let scaler = ImageIoScaler()
    private let cover = ImageTarget.cover(width: 800, height: 800)

    func testAnimatedGifPassesThroughByteForByte() throws {
        let frames = [
            Fixture.image(width: 1000, height: 1000, left: Fixture.red, right: Fixture.blue),
            Fixture.image(width: 1000, height: 1000, left: Fixture.blue, right: Fixture.red),
        ]
        let gif = try XCTUnwrap(Fixture.encode(
            frames,
            as: .gif,
            frameProperties: [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFDelayTime: 0.1]],
            containerProperties: [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFLoopCount: 0]]
        ))
        let scaled = try XCTUnwrap(scaler.scale(bytes: gif, sniff: sniffed(.gif, "image/gif", animated: true), target: cover))

        XCTAssertEqual(scaled.bytes, gif)
        XCTAssertEqual(scaled.mime, "image/gif")
        XCTAssertEqual(scaled.width, 1000)
        XCTAssertEqual(scaled.height, 1000)
    }

    func testApngPassesThroughByteForByte() throws {
        let frames = [
            Fixture.image(width: 1200, height: 900, left: Fixture.red, right: Fixture.blue),
            Fixture.image(width: 1200, height: 900, left: Fixture.blue, right: Fixture.red),
        ]
        let apng = try XCTUnwrap(Fixture.encode(
            frames,
            as: .png,
            frameProperties: [kCGImagePropertyPNGDictionary: [kCGImagePropertyAPNGDelayTime: 0.1]],
            containerProperties: [kCGImagePropertyPNGDictionary: [kCGImagePropertyAPNGLoopCount: 0]]
        ))
        let scaled = try XCTUnwrap(scaler.scale(bytes: apng, sniff: sniffed(.png, "image/png", animated: true), target: cover))

        XCTAssertEqual(scaled.bytes, apng)
        XCTAssertEqual(scaled.mime, "image/png")
        XCTAssertEqual(scaled.width, 1200)
        XCTAssertEqual(scaled.height, 900)
    }

    func testHeicStillLandsAsPlannedJpeg() throws {
        let still = Fixture.image(width: 3000, height: 2000, left: Fixture.red, right: Fixture.blue)
        guard let heic = Fixture.encode([still], as: .heic) else {
            throw XCTSkip("host cannot encode heic")
        }
        let scaled = try XCTUnwrap(scaler.scale(bytes: heic, sniff: sniffed(.heif, "image/heic"), target: cover))

        XCTAssertEqual(scaled.mime, "image/jpeg")
        XCTAssertEqual(scaled.width, 1200)
        XCTAssertEqual(scaled.height, 800)
        let decoded = Fixture.decode(scaled.bytes)
        XCTAssertEqual(decoded.type, UTType.jpeg.identifier)
        XCTAssertEqual(decoded.image.width, 1200)
        XCTAssertEqual(decoded.image.height, 800)
    }

    func testPngAlphaSurvivesADownscale() throws {
        let still = Fixture.image(width: 2000, height: 1000, left: Fixture.clear, right: Fixture.blue, alpha: true)
        let png = try XCTUnwrap(Fixture.encode([still], as: .png))

        let scaled = try XCTUnwrap(scaler.scale(bytes: png, sniff: sniffed(.png, "image/png"), target: cover))

        XCTAssertEqual(scaled.mime, "image/png")
        XCTAssertEqual(scaled.width, 1600)
        XCTAssertEqual(scaled.height, 800)
        let decoded = Fixture.decode(scaled.bytes)
        XCTAssertEqual(decoded.type, UTType.png.identifier)
        XCTAssertEqual(decoded.properties[kCGImagePropertyHasAlpha] as? Bool, true)
        XCTAssertEqual(decoded.image.width, 1600)
        XCTAssertEqual(decoded.image.height, 800)
        XCTAssertEqual(Fixture.pixel(decoded.image, x: 200, y: 400).alpha, 0)
        XCTAssertEqual(Fixture.pixel(decoded.image, x: 1400, y: 400), Rgba(red: 0, green: 0, blue: 255, alpha: 255))
    }

    func testLargeJpegShrinksToCoverTheBox() throws {
        let still = Fixture.image(width: 4032, height: 3024, left: Fixture.red, right: Fixture.blue)
        let jpeg = try XCTUnwrap(Fixture.encode([still], as: .jpeg))

        let scaled = try XCTUnwrap(scaler.scale(bytes: jpeg, sniff: sniffed(.jpeg, "image/jpeg"), target: cover))

        XCTAssertEqual(scaled.mime, "image/jpeg")
        XCTAssertEqual(scaled.width, 1067)
        XCTAssertEqual(scaled.height, 800)
        let decoded = Fixture.decode(scaled.bytes)
        XCTAssertEqual(decoded.type, UTType.jpeg.identifier)
        XCTAssertEqual(decoded.image.width, 1067)
        XCTAssertEqual(decoded.image.height, 800)
    }

    func testSmallJpegIsKeptUntouched() throws {
        let still = Fixture.image(width: 640, height: 480, left: Fixture.red, right: Fixture.blue)
        let jpeg = try XCTUnwrap(Fixture.encode([still], as: .jpeg))

        let scaled = try XCTUnwrap(scaler.scale(bytes: jpeg, sniff: sniffed(.jpeg, "image/jpeg"), target: cover))

        XCTAssertEqual(scaled.bytes, jpeg)
        XCTAssertEqual(scaled.mime, "image/jpeg")
        XCTAssertEqual(scaled.width, 640)
        XCTAssertEqual(scaled.height, 480)
    }

    func testExifRotatedJpegComesOutUprightWithoutAnOrientationTag() throws {
        let stored = Fixture.image(width: 2000, height: 1000, left: Fixture.red, right: Fixture.blue)
        let jpeg = try XCTUnwrap(Fixture.encode([stored], as: .jpeg, frameProperties: [kCGImagePropertyOrientation: 6]))

        let scaled = try XCTUnwrap(scaler.scale(bytes: jpeg, sniff: sniffed(.jpeg, "image/jpeg"), target: cover))

        XCTAssertEqual(scaled.mime, "image/jpeg")
        XCTAssertEqual(scaled.width, 800)
        XCTAssertEqual(scaled.height, 1600)
        let decoded = Fixture.decode(scaled.bytes)
        XCTAssertEqual(decoded.image.width, 800)
        XCTAssertEqual(decoded.image.height, 1600)
        XCTAssertEqual(decoded.properties[kCGImagePropertyOrientation] as? Int ?? 1, 1)
        let top = Fixture.pixel(decoded.image, x: 400, y: 200)
        let bottom = Fixture.pixel(decoded.image, x: 400, y: 1400)
        XCTAssertGreaterThan(top.red, 200)
        XCTAssertLessThan(top.blue, 60)
        XCTAssertGreaterThan(bottom.blue, 200)
        XCTAssertLessThan(bottom.red, 60)
    }

    func testThumbnailCapsTheLongEdgeAsAnUprightJpeg() throws {
        let stored = Fixture.image(width: 2000, height: 1000, left: Fixture.red, right: Fixture.blue)
        let jpeg = try XCTUnwrap(Fixture.encode([stored], as: .jpeg, frameProperties: [kCGImagePropertyOrientation: 6]))

        let scaled = try XCTUnwrap(scaler.scale(bytes: jpeg, sniff: sniffed(.jpeg, "image/jpeg"), target: .thumbnail(maxEdge: 512, quality: 0.6)))

        XCTAssertEqual(scaled.mime, "image/jpeg")
        XCTAssertEqual(scaled.width, 256)
        XCTAssertEqual(scaled.height, 512)
        let decoded = Fixture.decode(scaled.bytes)
        XCTAssertEqual(decoded.type, UTType.jpeg.identifier)
        XCTAssertEqual(decoded.image.width, 256)
        XCTAssertEqual(decoded.image.height, 512)
    }

    func testThumbnailReencodesAnAnimatedGifAsJpeg() throws {
        let frames = [
            Fixture.image(width: 1000, height: 600, left: Fixture.red, right: Fixture.blue),
            Fixture.image(width: 1000, height: 600, left: Fixture.blue, right: Fixture.red),
        ]
        let gif = try XCTUnwrap(Fixture.encode(
            frames,
            as: .gif,
            frameProperties: [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFDelayTime: 0.1]]
        ))

        let scaled = try XCTUnwrap(scaler.scale(
            bytes: gif, sniff: sniffed(.gif, "image/gif", animated: true), target: .thumbnail(maxEdge: 512, quality: 0.6)
        ))

        XCTAssertEqual(scaled.mime, "image/jpeg")
        XCTAssertEqual(scaled.width, 512)
        XCTAssertEqual(scaled.height, 307)
        XCTAssertEqual(Fixture.decode(scaled.bytes).type, UTType.jpeg.identifier)
    }

    func testThumbnailAndCoverDecodeTheHeifPrimaryItem() throws {
        let items = [
            Fixture.image(width: 64, height: 64, left: Fixture.red, right: Fixture.red),
            Fixture.image(width: 64, height: 64, left: Fixture.blue, right: Fixture.blue),
        ]
        guard let stored = Fixture.encode(items, as: .heic) else {
            throw XCTSkip("host cannot encode heic")
        }
        let heic = try XCTUnwrap(Fixture.withPrimaryItem(2, in: stored))
        let source = try XCTUnwrap(CGImageSourceCreateWithData(heic as CFData, nil))
        XCTAssertEqual(CGImageSourceGetPrimaryImageIndex(source), 1)

        for target in [ImageTarget.thumbnail(maxEdge: 512, quality: 0.9), cover] {
            let scaled = try XCTUnwrap(scaler.scale(bytes: heic, sniff: sniffed(.heif, "image/heic"), target: target))
            let center = Fixture.pixel(Fixture.decode(scaled.bytes).image, x: 32, y: 32)
            XCTAssertGreaterThan(center.blue, 200, "\(target)")
            XCTAssertLessThan(center.red, 60, "\(target)")
        }
    }

    func testCoverTrustsTheSniffItIsHandedInsteadOfSniffingAgain() throws {
        let jpeg = try XCTUnwrap(Fixture.encode(
            [Fixture.image(width: 1200, height: 900, left: Fixture.red, right: Fixture.blue)],
            as: .jpeg
        ))
        XCTAssertNil(scaler.scale(bytes: jpeg, sniff: unknown, target: cover))
        XCTAssertNotNil(scaler.scale(bytes: jpeg, sniff: sniffed(.jpeg, "image/jpeg"), target: cover))
    }

    func testBytesThatDoNotDecodeAreRefusedForCover() {
        let words = Data("not an image".utf8)
        XCTAssertNil(scaler.scale(bytes: words, sniff: sniffed(.jpeg, "image/jpeg"), target: cover))
    }
}
