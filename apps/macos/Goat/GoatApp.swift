import SwiftUI

@main
struct GoatApp: App {
  @State private var model = AppModel()

  var body: some Scene {
    MenuBarExtra {
      MenuBarContent(model: model)
    } label: {
      Image(model.computerUse.isActing ? "MenuBarIconActive" : "MenuBarIcon")
        .accessibilityLabel("Goat")
    }
    .menuBarExtraStyle(.window)
  }
}
