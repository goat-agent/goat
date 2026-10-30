import Computer
import CoreGraphics
import Foundation
import GoatAPI
import Observation

/// Answers the server's computer calls with this Mac, when the user allows it. It is also what
/// the menu bar and the borrowing indicator watch.
@Observable
final class ComputerService {
  private static let enabledKey = "isComputerUseEnabled"

  let permissions = ComputerPermissions()
  private(set) var actionsInProgress = 0
  /// The app whose window has the user's pointer while the agent borrows the screen.
  private(set) var borrowedApp: String?

  var isEnabled: Bool {
    didSet { UserDefaults.standard.set(isEnabled, forKey: Self.enabledKey) }
  }

  var isAvailable: Bool { isEnabled && permissions.areGranted }
  var isActing: Bool { actionsInProgress > 0 }

  @ObservationIgnored private lazy var computer = Computer { [weak self] app in
    await self?.setBorrowedApp(app)
  }
  @ObservationIgnored private let indicator = BorrowIndicator()

  init() {
    isEnabled = UserDefaults.standard.bool(forKey: Self.enabledKey)
  }

  /// Stops what the agent is doing on this Mac and switches computer use off until the user turns
  /// it back on.
  func stop() {
    isEnabled = false
    Task { await computer.stop() }
  }

  /// Ends a borrow without switching computer use off; Esc does this.
  func interrupt() {
    Task { await computer.stop() }
  }

  /// The JSON-RPC answer to one request from the server, or nil when it is not a request.
  func answer(_ data: Data) async -> Data? {
    let decoder = JSONDecoder()
    guard let envelope = try? decoder.decode(Envelope.self, from: data) else { return nil }
    let response: Response
    do {
      response = .success(id: envelope.id, result: try await perform(envelope.method, data))
    } catch let error as UnknownMethod {
      response = .failure(id: envelope.id, code: -32601, message: error.localizedDescription)
    } catch {
      response = .failure(id: envelope.id, code: -32000, message: error.localizedDescription)
    }
    return try? JSONEncoder().encode(response)
  }

  private func perform(_ method: String, _ data: Data) async throws -> any Encodable & Sendable {
    if method == "computer.status" {
      permissions.refresh()
      return Components.Schemas.ComputerStatus(isAvailable: isAvailable)
    }
    permissions.refresh()
    guard isAvailable else { throw ComputerError.notAllowed }
    actionsInProgress += 1
    defer { actionsInProgress -= 1 }

    switch method {
    case "computer.screen":
      let params = try decode(Components.Schemas.ComputerScreenParams.self, data)
      return try await computer.screen(includeImage: params.includeImage)
    case "computer.view":
      let params = try decode(Components.Schemas.ComputerViewParams.self, data)
      let region = params.region.map {
        CGRect(x: $0.x, y: $0.y, width: $0.width, height: $0.height)
      }
      return try await computer.view(
        window: params.window, region: region, waitSeconds: params.waitSeconds)
    case "computer.click":
      let params = try decode(Components.Schemas.ComputerClickParams.self, data)
      return try await computer.click(
        params.target, button: params.button.map { $0.value1?.rawValue ?? $0.value2 ?? "" },
        count: params.count,
        modifiers: (params.modifiers ?? []).compactMap { $0.value1?.rawValue ?? $0.value2 })
    case "computer.hover":
      let params = try decode(Components.Schemas.ComputerHoverParams.self, data)
      return try await computer.hover(params.target)
    case "computer.drag":
      let params = try decode(Components.Schemas.ComputerDragParams.self, data)
      return try await computer.drag(
        params.from, to: params.to,
        modifiers: (params.modifiers ?? []).compactMap { $0.value1?.rawValue ?? $0.value2 })
    case "computer.scroll":
      let params = try decode(Components.Schemas.ComputerScrollParams.self, data)
      return try await computer.scroll(
        params.target,
        direction: params.direction.value1?.rawValue ?? params.direction.value2 ?? "",
        amount: params.amount)
    case "computer.type":
      let params = try decode(Components.Schemas.ComputerTypeParams.self, data)
      return try await computer.type(params.target, text: params.text)
    case "computer.key":
      let params = try decode(Components.Schemas.ComputerKeyParams.self, data)
      return try await computer.key(params.target, keys: params.keys, repeating: params._repeat)
    case "computer.set":
      let params = try decode(Components.Schemas.ComputerSetParams.self, data)
      return try await computer.set(params.target, value: params.value)
    case "computer.menu":
      let params = try decode(Components.Schemas.ComputerMenuParams.self, data)
      return try await computer.menu(window: params.window, path: params.path)
    case "computer.open":
      let params = try decode(Components.Schemas.ComputerOpenParams.self, data)
      return try await computer.open(app: params.app, path: params.path, url: params.url)
    case "computer.clipboard":
      let params = try decode(Components.Schemas.ComputerClipboardParams.self, data)
      return await computer.clipboard(text: params.text, path: params.path)
    default:
      throw UnknownMethod(method: method)
    }
  }

  private func decode<Params: Decodable>(_ type: Params.Type, _ data: Data) throws -> Params {
    try JSONDecoder().decode(Request<Params>.self, from: data).params
  }

  private func setBorrowedApp(_ app: String?) {
    borrowedApp = app
    if app == nil {
      indicator.hide()
    } else {
      indicator.show { [weak self] in self?.interrupt() }
    }
  }
}

private struct Envelope: Decodable {
  let id: String
  let method: String
}

private struct Request<Params: Decodable>: Decodable {
  let params: Params
}

private struct UnknownMethod: LocalizedError {
  let method: String
  var errorDescription: String? { "This Mac does not know \(method). Update Goat for Mac." }
}

private enum Response: Encodable {
  case success(id: String, result: any Encodable & Sendable)
  case failure(id: String, code: Int, message: String)

  private enum CodingKeys: String, CodingKey {
    case jsonrpc, id, result, error
  }

  func encode(to encoder: any Encoder) throws {
    var container = encoder.container(keyedBy: CodingKeys.self)
    try container.encode("2.0", forKey: .jsonrpc)
    switch self {
    case .success(let id, let result):
      try container.encode(id, forKey: .id)
      try container.encode(result, forKey: .result)
    case .failure(let id, let code, let message):
      try container.encode(id, forKey: .id)
      try container.encode(
        Components.Schemas.RpcError(code: code, message: message), forKey: .error)
    }
  }
}
