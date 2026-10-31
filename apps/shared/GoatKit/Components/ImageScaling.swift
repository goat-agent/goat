import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// Shrinks images with ImageIO, which decodes only as many pixels as the result needs and
/// applies the photo's orientation.
nonisolated enum ImageScaling {
  @concurrent static func thumbnail(at url: URL, maxPixels: CGFloat) async -> CGImage? {
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
    return thumbnail(of: source, longEdge: maxPixels)
  }

  static func thumbnail(of data: Data, longEdge: CGFloat) -> CGImage? {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
    return thumbnail(of: source, longEdge: longEdge)
  }

  static func thumbnail(of source: CGImageSource, longEdge: CGFloat) -> CGImage? {
    let options: [CFString: Any] = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,
      kCGImageSourceThumbnailMaxPixelSize: max(1, Int(longEdge.rounded())),
    ]
    return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
  }

  /// The pixel size of the image as it is shown, after its orientation is applied.
  static func size(of source: CGImageSource) -> CGSize? {
    guard
      let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
      let width = properties[kCGImagePropertyPixelWidth] as? CGFloat,
      let height = properties[kCGImagePropertyPixelHeight] as? CGFloat
    else { return nil }
    let orientation = properties[kCGImagePropertyOrientation] as? UInt32 ?? 1
    // Orientations 5 to 8 turn the image a quarter, which swaps its sides.
    return orientation >= 5
      ? CGSize(width: height, height: width) : CGSize(width: width, height: height)
  }

  static func jpeg(_ image: CGImage, quality: CGFloat) -> Data? {
    let data = NSMutableData()
    guard
      let destination = CGImageDestinationCreateWithData(
        data, UTType.jpeg.identifier as CFString, 1, nil)
    else { return nil }
    let options: [CFString: Any] = [kCGImageDestinationLossyCompressionQuality: quality]
    CGImageDestinationAddImage(destination, image, options as CFDictionary)
    return CGImageDestinationFinalize(destination) ? data as Data : nil
  }
}
