import SwiftUI

@main
struct GoatApp: App {
  @State private var model = AppModel()

  var body: some Scene {
    WindowGroup {
      RootView(model: model)
    }
  }
}
