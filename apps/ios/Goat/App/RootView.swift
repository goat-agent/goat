import SwiftUI

struct RootView: View {
  let model: AppModel

  var body: some View {
    Group {
      switch model.screen {
      case .loading:
        if let message = model.errorMessage {
          ContentUnavailableView {
            Label(message, systemImage: "wifi.exclamationmark")
          } actions: {
            Button("다시 시도") { Task { await model.start() } }
              .buttonStyle(.glassProminent)
          }
        } else {
          ProgressView()
        }
      case .signIn:
        SignInFlow(model: model)
      case .setup:
        SetupFlow(model: model)
      case .home:
        AgentListView(model: model)
      }
    }
    .animation(.default, value: model.screen)
    .sensoryFeedback(.success, trigger: model.signIns)
    .task { await model.start() }
  }
}
