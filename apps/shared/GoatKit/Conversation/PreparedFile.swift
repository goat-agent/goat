import Foundation
import ImageIO
import UniformTypeIdentifiers

/// A file ready to upload: photos become JPEGs no larger than the agent can see.
nonisolated struct PreparedFile: Sendable {
  /// The largest image the model reads without shrinking it: a 2,576 px long edge and 4,784
  /// visual tokens of 28 × 28 px.
  private static let maxLongEdge: CGFloat = 2576
  private static let maxPixels: CGFloat = 4784 * 28 * 28

  let name: String
  let data: Data

  /// Reads a file the user picked in Files, which hands out security-scoped URLs.
  @concurrent static func file(at url: URL) async throws -> PreparedFile {
    let isScoped = url.startAccessingSecurityScopedResource()
    defer { if isScoped { url.stopAccessingSecurityScopedResource() } }
    let data = try Data(contentsOf: url)
    if UTType(filenameExtension: url.pathExtension)?.conforms(to: .image) == true,
      let photo = jpeg(from: data, named: url.deletingPathExtension().lastPathComponent)
    {
      return photo
    }
    return PreparedFile(name: url.lastPathComponent, data: data)
  }

  @concurrent static func photo(_ data: Data) async -> PreparedFile? {
    jpeg(from: data, named: "Photo")
  }

  private static func jpeg(from data: Data, named name: String) -> PreparedFile? {
    guard let source = CGImageSourceCreateWithData(data as CFData, nil),
      let size = ImageScaling.size(of: source)
    else { return nil }
    let scale = min(
      1,
      maxLongEdge / max(size.width, size.height),
      (maxPixels / (size.width * size.height)).squareRoot()
    )
    guard
      let image = ImageScaling.thumbnail(
        of: source, longEdge: max(size.width, size.height) * scale),
      let jpeg = ImageScaling.jpeg(image, quality: 0.85)
    else { return nil }
    return PreparedFile(name: "\(name).jpg", data: jpeg)
  }
}
