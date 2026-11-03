import AuthenticationServices
import SwiftUI
import WebKit

/// The main window: the web app, edge to edge. It also keeps this Mac's link to the server open
/// while someone is signed in, and offers computer use once after the first sign-in.
struct RootView: View {
  let webApp: WebApp
  let session: Session
  let computer: ComputerService
  let connection: DeviceConnection

  @AppStorage("didOfferComputerUse") private var didOfferComputerUse = false
  @Environment(\.openWindow) private var openWindow
  @Environment(\.webAuthenticationSession) private var authentication

  var body: some View {
    WebView(webApp.page)
      .webViewContentBackground(.hidden)
      .ignoresSafeArea()
      .frame(minWidth: 720, minHeight: 480)
      .containerBackground(Palette.sidebar, for: .window)
      .onAppear { webApp.requests.authentication = authentication }
      .onChange(of: session.isSignedIn, initial: true) { _, isSignedIn in
        if isSignedIn {
          connection.start(serverURL: .goatServer, token: { session.token }, service: computer)
          offerComputerUse()
        } else {
          connection.stop()
        }
      }
  }

  private func offerComputerUse() {
    guard !didOfferComputerUse else { return }
    didOfferComputerUse = true
    openWindow(id: GoatApp.permissionsWindow)
  }
}
