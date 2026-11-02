import SwiftUI

struct RootView: View {
  let model: AppModel

  var body: some View {
    Group {
      switch model.screen {
      case .loading:
        Group {
          if let message = model.errorMessage {
            VStack(spacing: 16) {
              Text(verbatim: message)
                .font(.headline)
                .foregroundStyle(Palette.foreground)
                .multilineTextAlignment(.center)
              Button("Try Again") { Task { await model.start() } }
                .buttonStyle(.primary)
            }
            .padding(32)
          } else {
            ProgressView()
          }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Palette.background)
      case .signIn:
        SignInFlow(model: model)
      case .setup:
        FirstAgentView(model: model)
      case .home:
        AgentListView(model: model)
      }
    }
    .portalHost()
    .environment(model)
    .animation(.default, value: model.screen)
    .sensoryFeedback(.success, trigger: model.signIns)
    .task { await model.start() }
  }
}
