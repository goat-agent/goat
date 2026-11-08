import AppKit
import ApplicationServices
import CoreGraphics

/// The two grants computer use needs. macOS does not announce changes to them, so they are read
/// again every few seconds while one is missing.
@Observable
final class Permissions {
  enum Kind {
    case accessibility, screenRecording
  }

  private(set) var hasAccessibility = AXIsProcessTrusted()
  private(set) var hasScreenRecording = CGPreflightScreenCaptureAccess()
  private var polling: Task<Void, Never>?

  var areGranted: Bool { hasAccessibility && hasScreenRecording }

  init() {
    pollWhileMissing()
  }

  func request(_ kind: Kind) {
    switch kind {
    case .accessibility:
      // The value of kAXTrustedCheckOptionPrompt, a global Swift 6 will not read from here.
      AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary)
    case .screenRecording:
      CGRequestScreenCaptureAccess()
    }
    openSettings(for: kind)
    pollWhileMissing()
  }

  private func openSettings(for kind: Kind) {
    let pane =
      switch kind {
      case .accessibility: "Privacy_Accessibility"
      case .screenRecording: "Privacy_ScreenCapture"
      }
    if let url = URL(
      string: "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?\(pane)")
    {
      NSWorkspace.shared.open(url)
    }
  }

  private func pollWhileMissing() {
    guard polling == nil, !areGranted else { return }
    polling = Task {
      while !areGranted {
        try? await Task.sleep(for: .seconds(2))
        hasAccessibility = AXIsProcessTrusted()
        hasScreenRecording = CGPreflightScreenCaptureAccess()
      }
      polling = nil
    }
  }
}
