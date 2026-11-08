import Darwin
import Foundation

/// How an app takes input from the background, which follows from what it is built with.
enum AppKind: Sendable {
  /// AppKit and WebKit: events and accessibility both work.
  case native
  /// Chrome and Electron: they stop drawing while fully covered, and ⌘ shortcuts reach them only in
  /// front.
  case chromium
  /// Mac Catalyst and iPad apps: they ignore pointer events sent to them.
  case uikit

  static func of(_ pid: pid_t) -> AppKind {
    guard let bundle = bundleURL(of: pid) else { return .native }
    let info = bundle.appending(path: "Contents/Info.plist")
    if let plist = NSDictionary(contentsOf: info), plist["UIDeviceFamily"] != nil { return .uikit }
    return hasRendererHelper(bundle) ? .chromium : .native
  }

  private static func bundleURL(of pid: pid_t) -> URL? {
    var buffer = [CChar](repeating: 0, count: Int(MAXPATHLEN))
    guard proc_pidpath(pid, &buffer, UInt32(buffer.count)) > 0 else { return nil }
    let path = String(
      decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
    var url = URL(fileURLWithPath: path)
    while url.pathComponents.count > 1 {
      if url.pathExtension == "app" { return url }
      url.deleteLastPathComponent()
    }
    return nil
  }

  private static func hasRendererHelper(_ bundle: URL) -> Bool {
    let frameworks = bundle.appending(path: "Contents/Frameworks")
    guard
      let walker = FileManager.default.enumerator(
        at: frameworks, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
    else { return false }
    for case let url as URL in walker {
      if url.lastPathComponent.hasSuffix("Helper (Renderer).app") { return true }
      if url.pathExtension == "app" || walker.level > 5 { walker.skipDescendants() }
    }
    return false
  }
}
