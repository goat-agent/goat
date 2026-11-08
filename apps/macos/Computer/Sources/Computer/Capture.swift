import CoreGraphics
import Foundation
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

enum Capture {
  /// The long edge the model sees. Detail beyond one pixel per point is never kept.
  static let maxLongEdge: CGFloat = 1280

  /// The whole window at full resolution, even when other windows cover it.
  static func window(_ id: CGWindowID) async throws -> CGImage {
    let content = try await SCShareableContent.excludingDesktopWindows(
      false, onScreenWindowsOnly: false)
    guard let window = content.windows.first(where: { $0.windowID == id }) else {
      throw ComputerError.windowGone
    }
    let filter = SCContentFilter(desktopIndependentWindow: window)
    let configuration = SCStreamConfiguration()
    let scale = CGFloat(filter.pointPixelScale)
    configuration.width = Int(filter.contentRect.width * scale)
    configuration.height = Int(filter.contentRect.height * scale)
    configuration.showsCursor = false
    configuration.ignoreShadowsSingleWindow = true
    return try await SCScreenshotManager.captureImage(
      contentFilter: filter, configuration: configuration)
  }

  static func sizeForModel(of image: CGImage, points: CGSize) -> CGSize {
    let longEdge = min(maxLongEdge, max(points.width, points.height))
    let scale = min(1, longEdge / CGFloat(max(image.width, image.height)))
    return CGSize(
      width: max(1, (CGFloat(image.width) * scale).rounded()),
      height: max(1, (CGFloat(image.height) * scale).rounded()))
  }

  static func resized(_ image: CGImage, to size: CGSize) -> CGImage? {
    if Int(size.width) == image.width && Int(size.height) == image.height { return image }
    guard
      let context = CGContext(
        data: nil, width: Int(size.width), height: Int(size.height), bitsPerComponent: 8,
        bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
    else { return nil }
    context.interpolationQuality = .high
    context.draw(image, in: CGRect(origin: .zero, size: size))
    return context.makeImage()
  }

  static func jpeg(_ image: CGImage) -> Data? {
    let data = NSMutableData()
    guard
      let destination = CGImageDestinationCreateWithData(
        data, UTType.jpeg.identifier as CFString, 1, nil)
    else { return nil }
    let options: [CFString: Any] = [kCGImageDestinationLossyCompressionQuality: 0.8]
    CGImageDestinationAddImage(destination, image, options as CFDictionary)
    guard CGImageDestinationFinalize(destination) else { return nil }
    return data as Data
  }
}
