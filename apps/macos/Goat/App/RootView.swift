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
        if let message = model.errorMessage {
          ContentUnavailableView {
            Label(message, systemImage: "wifi.exclamationmark")
          } actions: {
            Button("Try Again") { Task { await model.start() } }
              .buttonStyle(.glassProminent)
          }
        } else {
          ProgressView()
        }
      case .signIn, .setup:
        OnboardingView(model: model)
      case .home:
        HomeView(model: model)
          .frame(minWidth: 720, idealWidth: 960, minHeight: 480, idealHeight: 640)
      }
    }
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
