import AppKit

/// Asks the user before the agent brings an app to the front. No answer counts as no.
enum ForegroundConsent {
  private static let timeout: TimeInterval = 20

  static func ask(_ app: String) -> Bool {
    let alert = NSAlert()
    alert.messageText = String(localized: "Let Goat bring \(app) to the front?")
    alert.informativeText = String(
      localized:
        "The agent needs \(app) in front for a moment. Your pointer and the app you were using come back right after."
    )
    alert.addButton(withTitle: String(localized: "Allow"))
    alert.addButton(withTitle: String(localized: "Don’t Allow"))
    NSApp.activate()
    let timer = Timer(timeInterval: timeout, repeats: false) { _ in
      MainActor.assumeIsolated { NSApp.abortModal() }
    }
    RunLoop.main.add(timer, forMode: .modalPanel)
    defer { timer.invalidate() }
    return alert.runModal() == .alertFirstButtonReturn
  }
}
