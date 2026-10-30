import AppKit

/// Opening apps, files, and links, and the clipboard. Everything opens without coming to the
/// front, so the user keeps working in what they had.
enum Opening {
  private static let appFolders = [
    "/Applications", "/System/Applications", "/System/Applications/Utilities",
    "/Applications/Utilities", NSHomeDirectory() + "/Applications",
  ]

  /// Opens what was asked and returns the process that shows it.
  static func open(app: String?, path: String?, url: String?) async throws -> pid_t {
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.activates = false
    let workspace = NSWorkspace.shared

    if let app {
      let location = try find(app)
      return try await workspace.openApplication(at: location, configuration: configuration)
        .processIdentifier
    }
    let target: URL? =
      if let path { URL(filePath: (path as NSString).expandingTildeInPath) } else {
        url.flatMap(URL.init(string:))
      }
    guard let target, let handler = workspace.urlForApplication(toOpen: target) else {
      throw ComputerError.nothingToOpen
    }
    return try await workspace.open(
      [target], withApplicationAt: handler, configuration: configuration
    )
    .processIdentifier
  }

  /// The app's frontmost window once it has one, or nil if none appears in time.
  static func firstWindow(of pid: pid_t, within limit: Duration) async throws -> CGWindowID? {
    let deadline = ContinuousClock.now + limit
    while ContinuousClock.now < deadline {
      if let window = WindowCatalog.all().first(where: { $0.pid == pid && !$0.isMinimized }) {
        return window.id
      }
      try await Task.sleep(for: .milliseconds(150))
    }
    return nil
  }

  static func copy(text: String?, path: String?) {
    let pasteboard = NSPasteboard.general
    pasteboard.clearContents()
    if let path {
      pasteboard.writeObjects([URL(filePath: (path as NSString).expandingTildeInPath) as NSURL])
    } else if let text {
      pasteboard.setString(text, forType: .string)
    }
  }

  static func pasteboard() -> (text: String?, files: [String]?) {
    let pasteboard = NSPasteboard.general
    let urls =
      pasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true])
      as? [URL] ?? []
    return (pasteboard.string(forType: .string), urls.isEmpty ? nil : urls.map(\.path))
  }

  private static func find(_ name: String) throws -> URL {
    if let byID = NSWorkspace.shared.urlForApplication(withBundleIdentifier: name) { return byID }
    let wanted = name.lowercased().replacingOccurrences(of: ".app", with: "")
    for folder in appFolders {
      let entries = (try? FileManager.default.contentsOfDirectory(atPath: folder)) ?? []
      if let match = entries.first(where: {
        $0.lowercased() == wanted + ".app"
      }) {
        return URL(filePath: folder).appending(path: match)
      }
    }
    throw ComputerError.appNotFound(name)
  }
}
