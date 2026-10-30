import SwiftUI

/// The menu under Goat's menu bar icon: what the agent is doing and how to stop it, the switch
/// that lets it use this Mac, and the way back to the window.
struct MenuBarContent: View {
  @Bindable var computer: ComputerService
  let isSignedIn: Bool
  @Environment(\.openWindow) private var openWindow

  var body: some View {
    if computer.isActing {
      Text(
        computer.borrowedApp.map { "Using \($0) on your screen" } ?? "Working on this Mac")
      Button("Stop") { computer.stop() }
      Divider()
    }
    if isSignedIn {
      Toggle("Let the Agent Use This Mac", isOn: $computer.isEnabled)
      Divider()
    }
    Button("Open Goat") {
      openWindow(id: GoatApp.mainWindow)
      NSApplication.shared.activate()
    }
    Button("Quit Goat") { NSApplication.shared.terminate(nil) }
      .keyboardShortcut("q")
  }
}
