import AppKit
import Bridge
import GoatAPI
import WebKit

private typealias Contract = Bridge.Components.Schemas

/// The web app that fills Goat's window. It loads when Goat starts and stays for Goat's whole
/// life, so the window opens already drawn, even after it was closed.
final class WebApp {
  let page: WebPage
  let requests: RequestHandler

  init(session: Session, computer: ComputerService) {
    guard let scheme = URLScheme(AppScheme.name) else {
      preconditionFailure("WebKit refused \(AppScheme.name) as a scheme")
    }
    requests = RequestHandler(computer: computer)

    var configuration = WebPage.Configuration()
    configuration.urlSchemeHandlers[scheme] = AppScheme(
      session: session, serverURL: .goatServer, source: .current)
    configuration.userContentController.addUserScript(
      WKUserScript(
        source: Self.hostScript, injectionTime: .atDocumentStart, forMainFrameOnly: true))
    configuration.userContentController.addScriptMessageHandler(
      requests, contentWorld: .page, name: "goat")
    // Dictation and calls; macOS still asks the user once for the microphone.
    configuration.deviceSensorAuthorization = .init { _, _, origin in
      origin.protocol == AppScheme.name ? .grant : .deny
    }

    page = WebPage(
      configuration: configuration, navigationDecider: ExternalLinks(),
      dialogPresenter: FilePicker())
    #if DEBUG
      page.isInspectable = true
    #endif
    _ = page.load(URL(string: "\(AppScheme.name)://app/"))
  }

  /// Tells the page about something that happened outside it, such as a menu command.
  func send(_ event: Bridge.Components.Schemas.EventName) {
    Task {
      _ = try? await page.callJavaScript(eventScript(event, Contract.Empty()))
    }
  }

  private static var hostScript: String {
    let device = CurrentDevice.signingIn
    let host = Contract.Host(
      platform: .macos, device: .init(installationId: device.installationId, name: device.name))
    do {
      return try Bridge.hostScript(host)
    } catch {
      preconditionFailure("The host does not encode: \(error)")
    }
  }
}

extension AppScheme.Source {
  /// The dev server when the build names one, and otherwise the web app built into Goat.
  static var current: Self {
    if let value = Bundle.main.object(forInfoDictionaryKey: "GoatWebURL") as? String,
      !value.isEmpty, let url = URL(string: value)
    {
      return .devServer(url)
    }
    guard let root = Bundle.main.url(forResource: "Web", withExtension: nil) else {
      preconditionFailure("Goat has no Web folder; build apps/web before a release build")
    }
    return .bundled(root)
  }
}

/// Keeps the page on the web app and opens every other address in the default browser. Frames
/// inside the page load where they are.
private struct ExternalLinks: WebPage.NavigationDeciding {
  private static let outsideSchemes: Set<String> = ["http", "https", "mailto", "tel"]

  mutating func decidePolicy(
    for action: WebPage.NavigationAction, preferences: inout WebPage.NavigationPreferences
  ) async -> WKNavigationActionPolicy {
    guard let url = action.request.url, let scheme = url.scheme,
      Self.outsideSchemes.contains(scheme), action.target?.isMainFrame != false
    else { return .allow }
    NSWorkspace.shared.open(url)
    return .cancel
  }
}

/// Answers the page's file inputs with the system's open panel.
private struct FilePicker: WebPage.DialogPresenting {
  func handleFileInputPrompt(
    parameters: WKOpenPanelParameters, initiatedBy frame: WebPage.FrameInfo
  ) async -> WebPage.FileInputPromptResult {
    let panel = NSOpenPanel()
    panel.allowsMultipleSelection = parameters.allowsMultipleSelection
    panel.canChooseDirectories = parameters.allowsDirectories
    let response =
      if let window = NSApp.keyWindow {
        await panel.beginSheetModal(for: window)
      } else {
        await panel.begin()
      }
    return response == .OK ? .selected(panel.urls) : .cancel
  }
}
