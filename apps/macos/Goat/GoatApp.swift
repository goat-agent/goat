import ServiceManagement
import SwiftUI

@main
struct GoatApp: App {
  static let mainWindow = "main"
  static let permissionsWindow = "permissions"

  @State private var model = AppModel()
  @State private var computer = ComputerService()
  @State private var connection = DeviceConnection()

  init() {
    #if !DEBUG
      // The agent can use this Mac only while Goat runs, so it starts with the user's session.
      try? SMAppService.mainApp.register()
    #endif
  }

  var body: some Scene {
    Window("Goat", id: Self.mainWindow) {
      RootView(model: model, computer: computer, connection: connection)
    }
    .defaultSize(width: 1000, height: 680)
    // The window's buttons sit on the sidebar; there is no title bar above it.
    .windowStyle(.hiddenTitleBar)
    // Small while signing in, then as large as the conversation wants.
    .windowResizability(.contentSize)

    Window("Use This Mac", id: Self.permissionsWindow) {
      PermissionsView(computer: computer)
    }
    .windowResizability(.contentSize)
    .restorationBehavior(.disabled)

    Settings {
      SettingsView(model: model, computer: computer)
    }

    MenuBarExtra {
      MenuBarContent(computer: computer, isSignedIn: model.screen == .home)
    } label: {
      Image(computer.isActing ? "MenuBarGoatActive" : "MenuBarGoat")
        .accessibilityLabel("Goat")
    }
  }
}
