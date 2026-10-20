#if canImport(ImageIO) && canImport(CoreGraphics) && canImport(UniformTypeIdentifiers)

    import BridgethingCompanionCore
    import CoreGraphics
    import Foundation
    import ImageIO
    import UniformTypeIdentifiers

    public final class ImageIoScaler: ImageScaler, @unchecked Sendable {
        private static let encodable: [StillFormat] = [.jpeg, .png, .gif]

        public init() {}

        public func scale(bytes: Data, sniff: MediaSniff, target: ImageTarget) -> ScaledImage? {
            switch target {
            case let .thumbnail(maxEdge, quality):
                thumbnail(bytes: bytes, maxEdge: maxEdge, quality: quality)
            case let .cover(width, height):
                cover(bytes: bytes, sniff: sniff, coverWidth: width, coverHeight: height)
            }
        }

        private func thumbnail(bytes: Data, maxEdge: UInt32, quality: Float) -> ScaledImage? {
            guard let source = CGImageSourceCreateWithData(bytes as CFData, nil),
                  let image = Self.upright(
                      source, at: CGImageSourceGetPrimaryImageIndex(source), maxPixelSize: Int(maxEdge)
                  ),
                  let jpeg = Self.encode(image, as: .jpeg, quality: quality)
            else { return nil }
            return ScaledImage(bytes: jpeg, mime: "image/jpeg", width: UInt32(image.width), height: UInt32(image.height))
        }

        private func cover(bytes: Data, sniff: MediaSniff, coverWidth: UInt32, coverHeight: UInt32) -> ScaledImage? {
            guard sniff.kind == .image, let sourceMime = sniff.mime,
                  let source = CGImageSourceCreateWithData(bytes as CFData, nil),
                  case let primary = CGImageSourceGetPrimaryImageIndex(source),
                  let stored = Self.storedSize(source, at: primary)
            else { return nil }
            if sniff.animated {
                return ScaledImage(bytes: bytes, mime: sourceMime, width: stored.width, height: stored.height)
            }
            guard let image = Self.upright(source, at: primary, maxPixelSize: Int(max(stored.width, stored.height))) else {
                return nil
            }
            let alpha = Self.hasAlpha(source, at: primary)
            let plan = planCover(
                format: sniff.format,
                width: UInt32(image.width),
                height: UInt32(image.height),
                hasAlpha: alpha,
                coverWidth: coverWidth,
                coverHeight: coverHeight,
                encodable: Self.encodable
            )
            switch plan {
            case .keep:
                return ScaledImage(bytes: bytes, mime: sourceMime, width: UInt32(image.width), height: UInt32(image.height))
            case let .encode(width, height, format, mime, quality):
                guard let resized = Self.resize(image, width: Int(width), height: Int(height), alpha: alpha),
                      let encoded = Self.encode(resized, as: format, quality: quality)
                else { return nil }
                return ScaledImage(bytes: encoded, mime: mime, width: width, height: height)
            }
        }

        private static func upright(_ source: CGImageSource, at index: Int, maxPixelSize: Int) -> CGImage? {
            let options: [CFString: Any] = [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: maxPixelSize,
            ]
            return CGImageSourceCreateThumbnailAtIndex(source, index, options as CFDictionary)
        }

        private static func storedSize(_ source: CGImageSource, at index: Int) -> (width: UInt32, height: UInt32)? {
            guard let properties = CGImageSourceCopyPropertiesAtIndex(source, index, nil) as? [CFString: Any],
                  let width = properties[kCGImagePropertyPixelWidth] as? Int,
                  let height = properties[kCGImagePropertyPixelHeight] as? Int,
                  width > 0, height > 0
            else { return nil }
            return (UInt32(width), UInt32(height))
        }

        private static func hasAlpha(_ source: CGImageSource, at index: Int) -> Bool {
            let options = [kCGImageSourceShouldCache: false] as CFDictionary
            switch CGImageSourceCreateImageAtIndex(source, index, options)?.alphaInfo {
            case nil, .none, .noneSkipFirst, .noneSkipLast:
                return false
            default:
                return true
            }
        }

        private static func resize(_ image: CGImage, width: Int, height: Int, alpha: Bool) -> CGImage? {
            guard let space = CGColorSpace(name: CGColorSpace.sRGB),
                  let context = CGContext(
                      data: nil,
                      width: width,
                      height: height,
                      bitsPerComponent: 8,
                      bytesPerRow: 0,
                      space: space,
                      bitmapInfo: (alpha ? CGImageAlphaInfo.premultipliedLast : .noneSkipLast).rawValue
                  )
            else { return nil }
            context.interpolationQuality = .high
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            return context.makeImage()
        }

        private static func encode(_ image: CGImage, as format: StillFormat, quality: Float) -> Data? {
            let type: UTType
            var options: [CFString: Any] = [:]
            switch format {
            case .jpeg:
                type = .jpeg
                options[kCGImageDestinationLossyCompressionQuality] = Double(min(max(quality, 0), 1))
            case .png:
                type = .png
            case .gif:
                type = .gif
            case .webp:
                return nil
            }
            let out = NSMutableData()
            guard let destination = CGImageDestinationCreateWithData(out, type.identifier as CFString, 1, nil) else {
                return nil
            }
            CGImageDestinationAddImage(destination, image, options as CFDictionary)
            guard CGImageDestinationFinalize(destination) else { return nil }
            return out as Data
        }
    }

#endif
