import Bridge
import ServiceManagement
import SwiftUI

@main
struct GoatApp: App {
  static let mainWindow = "main"
  static let permissionsWindow = "permissions"

  @State private var session: Session
  @State private var computer: ComputerService
  @State private var connection = DeviceConnection()
  @State private var webApp: WebApp

  init() {
    let session = Session()
    let computer = ComputerService()
    _session = State(initialValue: session)
    _computer = State(initialValue: computer)
    _webApp = State(initialValue: WebApp(session: session, computer: computer))
    #if !DEBUG
      // The agent can use this Mac only while Goat runs, so it starts with the user's session.
      try? SMAppService.mainApp.register()
    #endif
  }

  var body: some Scene {
    Window("Goat", id: Self.mainWindow) {
      RootView(webApp: webApp, session: session, computer: computer, connection: connection)
    }
    .defaultSize(width: 1000, height: 680)
    // The page draws the window's top: the window's buttons sit on its sidebar.
    .windowStyle(.hiddenTitleBar)
    .commands { AppCommands(webApp: webApp) }

    Window("Use This Mac", id: Self.permissionsWindow) {
      PermissionsView(computer: computer)
    }
    .windowResizability(.contentSize)
    .restorationBehavior(.disabled)

    MenuBarExtra {
      MenuBarContent(computer: computer, isSignedIn: session.isSignedIn)
    } label: {
      Image(computer.isActing ? "MenuBarGoatActive" : "MenuBarGoat")
        .accessibilityLabel("Goat")
    }
    .menuBarExtraStyle(.window)
  }
}

/// Settings live in the web app, so the menu command opens them there.
private struct AppCommands: Commands {
  let webApp: WebApp
  @Environment(\.openWindow) private var openWindow

  var body: some Commands {
    CommandGroup(replacing: .appSettings) {
      Button("Settings…") {
        openWindow(id: GoatApp.mainWindow)
        webApp.send(.openSettings)
      }
      .keyboardShortcut(",")
    }
  }
}
