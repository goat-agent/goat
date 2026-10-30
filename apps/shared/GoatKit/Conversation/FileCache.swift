import Foundation

/// Files on this device, kept under their server id so each is downloaded at most once. The
/// system may purge the caches directory; a missing file is downloaded again.
nonisolated enum FileCache {
  private static let directory = URL.cachesDirectory.appending(
    path: "Files", directoryHint: .isDirectory)

  static func existing(id: String, name: String) -> URL? {
    let url = location(id: id, name: name)
    return FileManager.default.fileExists(atPath: url.path(percentEncoded: false)) ? url : nil
  }

  /// Keeps a file that has no server id yet, so it can be previewed while it uploads.
  @concurrent static func stage(_ data: Data, name: String) async throws -> URL {
    try write(data, to: location(id: "staging-\(UUID().uuidString)", name: name))
  }

  @concurrent static func store(_ data: Data, id: String, name: String) async throws -> URL {
    try write(data, to: location(id: id, name: name))
  }

  @concurrent static func read(_ url: URL) async throws -> Data {
    try Data(contentsOf: url)
  }

  /// Moves a staged file under the id the server gave it.
  @concurrent static func adopt(_ staged: URL, id: String, name: String) async throws -> URL {
    let url = location(id: id, name: name)
    let manager = FileManager.default
    try manager.createDirectory(
      at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    try? manager.removeItem(at: url)
    try manager.moveItem(at: staged, to: url)
    try? manager.removeItem(at: staged.deletingLastPathComponent())
    return url
  }

  /// Quick Look shows the last path component as the title, so the file keeps its own name.
  private static func location(id: String, name: String) -> URL {
    let safeName = name.replacing("/", with: "-")
    return directory.appending(path: id, directoryHint: .isDirectory)
      .appending(path: ["", ".", ".."].contains(safeName) ? "file" : safeName)
  }

  private static func write(_ data: Data, to url: URL) throws -> URL {
    try FileManager.default.createDirectory(
      at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    try data.write(to: url, options: .atomic)
    return url
  }
}
