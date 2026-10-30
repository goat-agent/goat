import AppKit
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

/// Pictures of windows and the screen, sized for the model.
enum Capture {
  /// Measured on ScreenSpot-Pro's Mac screens: Opus 5.5 hits as often at 1280 as at the 2,560
  /// maximum, for about a third of the tokens.
  static let maxLongEdge: CGFloat = 1280

  /// The whole window at full resolution, even when other windows cover it.
  static func window(_ id: CGWindowID) async throws -> CGImage {
    let content = try await SCShareableContent.excludingDesktopWindows(
      false, onScreenWindowsOnly: false)
    guard let window = content.windows.first(where: { $0.windowID == id }) else {
      throw ComputerError.windowGone
    }
    let filter = SCContentFilter(desktopIndependentWindow: window)
    return try await image(of: filter)
  }

  /// The main display without Goat's own windows.
  static func screen() async throws -> CGImage {
    let content = try await SCShareableContent.excludingDesktopWindows(
      false, onScreenWindowsOnly: true)
    guard let display = content.displays.first(where: { $0.displayID == CGMainDisplayID() })
    else { throw ComputerError.noDisplay }
    let me = content.applications.filter {
      $0.processID == ProcessInfo.processInfo.processIdentifier
    }
    let filter = SCContentFilter(display: display, excludingApplications: me, exceptingWindows: [])
    return try await image(of: filter)
  }

  /// The image scaled so its long edge is at most `maxLongEdge` and never more than one pixel per
  /// point, which is how much detail the model can use.
  static func forModel(_ image: CGImage, points: CGSize) -> CGImage {
    let longEdge = min(maxLongEdge, max(points.width, points.height))
    let scale = longEdge / CGFloat(max(image.width, image.height))
    return scale < 1 ? resized(image, scale: scale) ?? image : image
  }

  static func crop(_ image: CGImage, to rect: CGRect) -> CGImage? {
    image.cropping(
      to: rect.integral.intersection(CGRect(x: 0, y: 0, width: image.width, height: image.height)))
  }

  static func jpeg(_ image: CGImage) -> String? {
    let data = NSMutableData()
    guard
      let destination = CGImageDestinationCreateWithData(
        data, UTType.jpeg.identifier as CFString, 1, nil)
    else { return nil }
    let options: [CFString: Any] = [kCGImageDestinationLossyCompressionQuality: 0.8]
    CGImageDestinationAddImage(destination, image, options as CFDictionary)
    guard CGImageDestinationFinalize(destination) else { return nil }
    return (data as Data).base64EncodedString()
  }

  private static func image(of filter: SCContentFilter) async throws -> CGImage {
    let configuration = SCStreamConfiguration()
    let scale = CGFloat(filter.pointPixelScale)
    configuration.width = Int(filter.contentRect.width * scale)
    configuration.height = Int(filter.contentRect.height * scale)
    configuration.showsCursor = false
    configuration.ignoreShadowsSingleWindow = true
    return try await SCScreenshotManager.captureImage(
      contentFilter: filter, configuration: configuration)
  }

  private static func resized(_ image: CGImage, scale: CGFloat) -> CGImage? {
    let width = max(1, Int((CGFloat(image.width) * scale).rounded()))
    let height = max(1, Int((CGFloat(image.height) * scale).rounded()))
    guard
      let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB) ?? CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
    else { return nil }
    context.interpolationQuality = .high
    context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    return context.makeImage()
  }
}
