import AppKit
import AuthenticationServices
import Bridge
import SwiftUI
import WebKit

private typealias Contract = Bridge.Components.Schemas

/// Answers what the page asks of the Mac: moving its window, a service's sign-in, and computer
/// use. The methods are the contract's; a new one does not compile until it is answered here.
final class RequestHandler: NSObject, WKScriptMessageHandlerWithReply {
  private let computer: ComputerService
  /// SwiftUI ties the sign-in sheet to a window, so the window's view hands it over.
  var authentication: WebAuthenticationSession?

  init(computer: ComputerService) {
    self.computer = computer
  }

  func userContentController(
    _ userContentController: WKUserContentController, didReceive message: WKScriptMessage
  ) async -> (Any?, String?) {
    guard let request = Request(body: message.body) else { return (nil, "not a request") }
    do {
      return (try await answer(request, window: message.webView?.window), nil)
    } catch {
      return (nil, String(describing: error))
    }
  }

  private func answer(_ request: Request, window: NSWindow?) async throws -> Any {
    switch request.method {
    case .moveWindow:
      if let window { move(window) }
      return try reply(Contract.Empty())
    case .doubleClickTitleBar:
      if let window { doubleClick(window) }
      return try reply(Contract.Empty())
    case .authenticate:
      let url = try request.params(Contract.AuthenticateParams.self).url
      return try reply(Contract.AuthenticateResult(callbackUrl: try await authenticate(url)))
    case .computerUse:
      return try reply(computerUse())
    case .setComputerUse:
      computer.isEnabled = try request.params(Contract.SetComputerUseParams.self).isEnabled
      return try reply(computerUse())
    case .openPermissionSettings:
      switch try request.params(Contract.OpenPermissionSettingsParams.self).permission {
      case .accessibility: computer.permissions.request(.accessibility)
      case .screenRecording: computer.permissions.request(.screenRecording)
      }
      return try reply(Contract.Empty())
    }
  }

  /// WebKit hands the page its mousedown after AppKit has finished with the event, so the drag
  /// starts from a fresh mouse-down where the pointer is, and only while the button is held.
  private func move(_ window: NSWindow) {
    guard NSEvent.pressedMouseButtons & 1 == 1,
      let event = NSEvent.mouseEvent(
        with: .leftMouseDown, location: window.mouseLocationOutsideOfEventStream, modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
        context: nil, eventNumber: 0, clickCount: 1, pressure: 1)
    else { return }
    window.performDrag(with: event)
  }

  /// Does what System Settings › Desktop & Dock says a double-clicked title bar does.
  private func doubleClick(_ window: NSWindow) {
    switch UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick") {
    case "Minimize": window.miniaturize(nil)
    case "None": break
    default: window.zoom(nil)
    }
  }

  /// The address the service's sign-in ended on, or nil when the user closed it.
  private func authenticate(_ address: String) async throws -> String? {
    guard let url = URL(string: address), let authentication else { throw URLError(.badURL) }
    do {
      let callback = try await authentication.authenticate(
        using: url, callbackURLScheme: "goat", preferredBrowserSession: .shared)
      return callback.absoluteString
    } catch let error as ASWebAuthenticationSessionError where error.code == .canceledLogin {
      return nil
    }
  }

  private func computerUse() -> Contract.ComputerUse {
    computer.permissions.refresh()
    return .init(
      isEnabled: computer.isEnabled,
      permissions: .init(
        accessibility: computer.permissions.canUseAccessibility,
        screenRecording: computer.permissions.canRecordScreen))
  }
}
