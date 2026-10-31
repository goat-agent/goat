import SwiftUI

@main
struct GoatApp: App {
  @State private var model = AppModel()
  @UIApplicationDelegateAdaptor private var delegate: AppDelegate

  var body: some Scene {
    WindowGroup {
      RootView(model: model)
        .onAppear { delegate.model = model }
    }
  }
}
