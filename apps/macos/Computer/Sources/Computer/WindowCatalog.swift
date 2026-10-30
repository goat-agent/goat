import AppKit
import ApplicationServices

struct WindowInfo {
  let id: CGWindowID
  let pid: pid_t
  let app: String
  let title: String
  /// Global points, origin at the top left of the main display.
  let frame: CGRect
  let isMinimized: Bool
}

/// The windows the agent may use: other apps' normal windows. Goat's own windows are left out,
/// so the agent cannot switch off its own limits.
enum WindowCatalog {
  private static let minimumSide: CGFloat = 40

  /// Windows on screen, front to back, then minimized ones.
  static func all() -> [WindowInfo] {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    let shown = (CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? [])
      .compactMap(info(from:))
    return shown + minimized()
  }

  static func window(_ id: CGWindowID) -> WindowInfo? {
    let list = CGWindowListCopyWindowInfo(.optionIncludingWindow, id) as? [[String: Any]] ?? []
    return list.first.flatMap(info(from:))
  }

  /// The accessibility element of a window, found among its app's windows.
  static func element(of window: WindowInfo) -> AXUIElement? {
    let app = AXUIElement.application(window.pid)
    let windows = (app.attribute(kAXWindowsAttribute) as? [AXUIElement]) ?? []
    return windows.first { $0.windowID == window.id }
  }

  private static func info(from entry: [String: Any]) -> WindowInfo? {
    guard (entry[kCGWindowLayer as String] as? Int) == 0,
      let id = entry[kCGWindowNumber as String] as? CGWindowID,
      let pid = entry[kCGWindowOwnerPID as String] as? pid_t,
      pid != ProcessInfo.processInfo.processIdentifier,
      let bounds = entry[kCGWindowBounds as String] as? NSDictionary,
      let frame = CGRect(dictionaryRepresentation: bounds),
      frame.width >= minimumSide, frame.height >= minimumSide
    else { return nil }
    let app = entry[kCGWindowOwnerName as String] as? String ?? ""
    // Window names need screen recording permission; the accessibility title does not.
    var title = entry[kCGWindowName as String] as? String ?? ""
    let isOnScreen = entry[kCGWindowIsOnscreen as String] as? Bool ?? false
    if title.isEmpty {
      let window = WindowInfo(
        id: id, pid: pid, app: app, title: "", frame: frame, isMinimized: false)
      title = element(of: window)?.string(kAXTitleAttribute) ?? ""
    }
    return WindowInfo(
      id: id, pid: pid, app: app, title: title, frame: frame, isMinimized: !isOnScreen)
  }

  private static func minimized() -> [WindowInfo] {
    let me = ProcessInfo.processInfo.processIdentifier
    return NSWorkspace.shared.runningApplications
      .filter { $0.activationPolicy == .regular && $0.processIdentifier != me && !$0.isHidden }
      .flatMap { app -> [WindowInfo] in
        let windows =
          (AXUIElement.application(app.processIdentifier).attribute(kAXWindowsAttribute)
            as? [AXUIElement]) ?? []
        return windows.compactMap { element in
          guard element.bool(kAXMinimizedAttribute) == true, let id = element.windowID,
            let frame = element.frame
          else { return nil }
          return WindowInfo(
            id: id, pid: app.processIdentifier, app: app.localizedName ?? "",
            title: element.string(kAXTitleAttribute) ?? "", frame: frame, isMinimized: true)
        }
      }
  }
}
