import CoreGraphics
import Foundation

/// Pictures for agents.
nonisolated enum AgentPhoto {
  /// The server keeps one size; every place an agent appears scales it down from this.
  static let side = 512

  /// A square JPEG cut from the middle of a picked photo, small enough to upload quickly.
  @concurrent static func prepare(_ data: Data) async -> Data? {
    guard let image = ImageScaling.thumbnail(of: data, longEdge: CGFloat(side * 2)) else {
      return nil
    }
    let length = min(image.width, image.height)
    let square = CGRect(
      x: (image.width - length) / 2, y: (image.height - length) / 2, width: length, height: length)
    guard let cropped = image.cropping(to: square),
      let context = CGContext(
        data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)
    else { return nil }
    context.interpolationQuality = .high
    context.draw(cropped, in: CGRect(x: 0, y: 0, width: side, height: side))
    return context.makeImage().flatMap { ImageScaling.jpeg($0, quality: 0.85) }
  }
}
