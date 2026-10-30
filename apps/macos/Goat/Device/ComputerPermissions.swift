import AppKit
import ApplicationServices
import Observation

/// The two macOS permissions computer use needs. macOS gives no notice when they change, so the
/// app reads them again whenever a screen that shows them is open.
@Observable
final class ComputerPermissions {
  enum Kind {
    case accessibility
    case screenRecording

    fileprivate var settingsURL: URL? {
      let pane =
        switch self {
        case .accessibility: "Privacy_Accessibility"
        case .screenRecording: "Privacy_ScreenCapture"
        }
      return URL(string: "x-apple.systempreferences:com.apple.preference.security?\(pane)")
    }
  }

  private(set) var canUseAccessibility = AXIsProcessTrusted()
  private(set) var canRecordScreen = CGPreflightScreenCaptureAccess()

  var areGranted: Bool { canUseAccessibility && canRecordScreen }

  func refresh() {
    canUseAccessibility = AXIsProcessTrusted()
    canRecordScreen = CGPreflightScreenCaptureAccess()
  }

  /// Adds Goat to the list in System Settings, then opens that list.
  func request(_ kind: Kind) {
    switch kind {
    case .accessibility:
      let options = ["AXTrustedCheckOptionPrompt": false] as CFDictionary
      _ = AXIsProcessTrustedWithOptions(options)
    case .screenRecording:
      _ = CGRequestScreenCaptureAccess()
    }
    if let url = kind.settingsURL { NSWorkspace.shared.open(url) }
  }

  /// Polls while the caller's task runs.
  func watch() async {
    while !Task.isCancelled {
      refresh()
      try? await Task.sleep(for: .seconds(1))
    }
  }
}
