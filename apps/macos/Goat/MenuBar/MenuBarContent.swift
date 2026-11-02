import SwiftUI

/// The small window under Goat's menu bar icon: what the agent is doing and how to stop it, the
/// switch that lets it use this Mac, and the way back to the window.
struct MenuBarContent: View {
  @Bindable var computer: ComputerService
  let isSignedIn: Bool
  @Environment(\.openWindow) private var openWindow

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      if computer.isActing {
        HStack(spacing: 10) {
          AgentFace(size: 28)
          VStack(alignment: .leading, spacing: 1) {
            Text("Using your screen")
              .font(.body.weight(.medium))
              .foregroundStyle(Palette.foreground)
            if let app = computer.borrowedApp {
              Text(verbatim: app)
                .font(.callout)
                .foregroundStyle(Palette.mutedForeground)
                .lineLimit(1)
            }
          }
          Spacer(minLength: 8)
          Button("Stop") { computer.stop() }
            .buttonStyle(.destructive)
            .controlSize(.small)
        }
        .padding(8)
        DropdownMenuSeparator()
      }
      if isSignedIn {
        Switch("Let the agent use this Mac", isOn: $computer.isEnabled)
          .foregroundStyle(Palette.foreground)
          .padding(.horizontal, 8)
          .frame(minHeight: 40)
        DropdownMenuSeparator()
      }
      DropdownMenuItem(title: "Open Goat") {
        openWindow(id: GoatApp.mainWindow)
        NSApplication.shared.activate()
      }
      DropdownMenuItem(title: "Quit Goat") { NSApplication.shared.terminate(nil) }
    }
    .padding(6)
    .frame(width: 280)
    .background(Palette.popover)
  }
}
