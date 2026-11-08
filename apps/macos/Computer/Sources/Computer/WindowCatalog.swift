import ApplicationServices
import Foundation

enum WindowCatalog {
  /// Ordinary windows of other apps: those on screen, front to back, then the minimized ones.
  static func all() -> [Window] {
    let listed = list([.optionAll, .excludeDesktopElements], relativeTo: kCGNullWindowID)
    let onScreen = listed.filter(\.isOnScreen)
    let minimized = listed.filter { !$0.isOnScreen && isMinimized($0) }
    return onScreen + minimized
  }

  static func window(_ id: CGWindowID) -> Window? {
    list([.optionIncludingWindow], relativeTo: id).first
  }

  /// The frames of on-screen windows in front of this one.
  static func framesAbove(_ window: Window) -> [CGRect] {
    list([.optionOnScreenAboveWindow, .excludeDesktopElements], relativeTo: window.id)
      .map(\.frame)
  }

  /// The window's accessibility element, matched by frame and title.
  static func element(of window: Window) -> AXUIElement? {
    let candidates = AXUIElement.application(window.pid).elements(kAXWindowsAttribute).filter {
      guard let frame = $0.frame else { return false }
      return abs(frame.minX - window.frame.minX) < 2 && abs(frame.minY - window.frame.minY) < 2
        && abs(frame.width - window.frame.width) < 2 && abs(frame.height - window.frame.height) < 2
    }
    return candidates.first { $0.string(kAXTitleAttribute) == window.title } ?? candidates.first
  }

  static func isMinimized(_ window: Window) -> Bool {
    element(of: window)?.bool(kAXMinimizedAttribute) ?? false
  }

  private static func list(_ options: CGWindowListOption, relativeTo id: CGWindowID) -> [Window] {
    let entries = CGWindowListCopyWindowInfo(options, id) as? [[String: Any]] ?? []
    let me = ProcessInfo.processInfo.processIdentifier
    return entries.compactMap { entry in
      guard (entry[kCGWindowLayer as String] as? Int) == 0,
        let number = entry[kCGWindowNumber as String] as? Int,
        let owner = entry[kCGWindowOwnerPID as String] as? Int, pid_t(owner) != me,
        let bounds = entry[kCGWindowBounds as String] as? NSDictionary,
        let frame = CGRect(dictionaryRepresentation: bounds),
        frame.width >= 40, frame.height >= 40,
        (entry[kCGWindowAlpha as String] as? Double ?? 1) > 0
      else { return nil }
      return Window(
        id: CGWindowID(number),
        pid: pid_t(owner),
        app: entry[kCGWindowOwnerName as String] as? String ?? "",
        title: entry[kCGWindowName as String] as? String ?? "",
        frame: frame,
        isOnScreen: entry[kCGWindowIsOnscreen as String] as? Bool ?? false
      )
    }
  }
}
