import SwiftUI

@main
struct GoatApp: App {
  @State private var model = AppModel()

  var body: some Scene {
    MenuBarExtra {
      MenuBarContent(model: model)
    } label: {
      Image(systemName: model.computerUse.isActing ? "cursorarrow.motionlines" : "cursorarrow")
        .accessibilityLabel("Goat")
    }
    .menuBarExtraStyle(.window)
  }
}
