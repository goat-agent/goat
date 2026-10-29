import SwiftUI

struct SetupFlow: View {
  @Bindable var model: AppModel

  var body: some View {
    NavigationStack(path: $model.setupPath) {
      Group {
        if model.needsName {
          NameView(model: model)
        } else {
          AgentSetupView(model: model)
        }
      }
      .navigationDestination(for: AppModel.SetupStep.self) { _ in
        AgentSetupView(model: model)
      }
    }
  }
}
