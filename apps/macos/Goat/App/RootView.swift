import SwiftUI

struct RootView: View {
  let model: AppModel
  let computer: ComputerService
  let connection: DeviceConnection

  @AppStorage("didOfferComputerUse") private var didOfferComputerUse = false
  @Environment(\.openWindow) private var openWindow

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
            .padding(48)
          } else {
            ProgressView()
              .controlSize(.small)
          }
        }
        .containerBackground(Palette.background, for: .window)
      case .signIn, .setup:
        OnboardingView(model: model)
      case .home:
        HomeView(model: model)
      }
    }
    .portalHost()
    .environment(model)
    .animation(.default, value: model.screen)
    .task { await model.start() }
    .onChange(of: model.screen, initial: true) { _, screen in
      if screen == .home {
        connection.start(
          serverURL: model.serverURL, token: { model.sessionToken }, service: computer)
        if !didOfferComputerUse {
          didOfferComputerUse = true
          openWindow(id: GoatApp.permissionsWindow)
        }
      } else if screen == .signIn {
        connection.stop()
      }
    }
  }
}
