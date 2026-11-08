import Computer
import CoreGraphics
import Foundation
import GoatAPI

/// Answers the server's `computer.*` calls: it checks that the user lets the agent use this Mac,
/// turns the call into a `Computer` request, and the result back into the server's types.
@Observable
final class ComputerUse {
  enum Failure: LocalizedError {
    case notAllowed
    case unknownMethod(String)
    case badParams(String)

    var errorDescription: String? {
      switch self {
      case .notAllowed: "The user has not allowed the agent to use this Mac."
      case .unknownMethod(let method): "This Mac does not know \(method)."
      case .badParams(let detail): detail
      }
    }
  }

  private static let enabledKey = "computer-use-enabled"

  var isEnabled = UserDefaults.standard.bool(forKey: ComputerUse.enabledKey) {
    didSet { UserDefaults.standard.set(isEnabled, forKey: Self.enabledKey) }
  }
  private(set) var isActing = false

  private let permissions: Permissions
  private let computer = Computer { app in await MainActor.run { ForegroundConsent.ask(app) } }
  private let decoder = JSONDecoder()
  private let encoder = JSONEncoder()

  init(permissions: Permissions) {
    self.permissions = permissions
  }

  var isAvailable: Bool { isEnabled && permissions.areGranted }

  /// The result of a call as JSON, or the error the server should report.
  func answer(_ method: String, _ request: Data) async throws -> Data {
    if method == "computer.status" {
      return try encoder.encode(Components.Schemas.ComputerStatus(isAvailable: isAvailable))
    }
    guard isAvailable else { throw Failure.notAllowed }
    switch method {
    case "computer.windows":
      let windows = await computer.windows()
      return try encoder.encode(Components.Schemas.ComputerWindows(windows: windows.map(\.schema)))
    case "computer.view":
      let call = try params(Components.Schemas.ComputerViewParams.self, request)
      let view = try await computer.view(
        window: call.window, region: call.region.map(\.rect),
        wait: call.waitSeconds.map { .milliseconds(Int($0 * 1000)) })
      return try encoder.encode(view.schema)
    case "computer.open":
      let call = try params(Components.Schemas.ComputerOpenParams.self, request)
      return try await acting {
        try await computer.open(call.target(), foreground: call.foreground ?? false)
      }
    default:
      let (action, window, foreground) = try action(method, request)
      return try await acting {
        try await computer.perform(action, window: window, foreground: foreground)
      }
    }
  }

  private func acting(_ body: () async throws -> Outcome) async throws -> Data {
    isActing = true
    defer { isActing = false }
    return try encoder.encode(try await body().schema)
  }

  private func action(_ method: String, _ request: Data) throws -> (Action, String, Bool) {
    switch method {
    case "computer.click":
      let call = try params(Components.Schemas.ComputerClickParams.self, request)
      let button: MouseButton =
        switch call.button?.value1 {
        case .right: .right
        case .middle: .middle
        case .left, nil: .left
        }
      return (
        .click(
          at: call.at.point, button: button, count: call.count ?? 1,
          modifiers: Modifiers(names: call.modifiers?.compactMap(\.value1?.rawValue))),
        call.window, call.foreground ?? false
      )
    case "computer.drag":
      let call = try params(Components.Schemas.ComputerDragParams.self, request)
      return (
        .drag(
          from: call.from.point, to: call.to.point,
          modifiers: Modifiers(names: call.modifiers?.compactMap(\.value1?.rawValue))),
        call.window, call.foreground ?? false
      )
    case "computer.scroll":
      let call = try params(Components.Schemas.ComputerScrollParams.self, request)
      let direction: ScrollDirection =
        switch call.direction.value1 {
        case .up: .up
        case .down: .down
        case .left: .left
        case .right: .right
        case nil: throw Failure.badParams("Scroll up, down, left, or right.")
        }
      return (
        .scroll(at: call.at.point, direction: direction, amount: call.amount), call.window,
        call.foreground ?? false
      )
    case "computer.move":
      let call = try params(Components.Schemas.ComputerMoveParams.self, request)
      return (.move(to: call.at.point), call.window, call.foreground ?? false)
    case "computer.type":
      let call = try params(Components.Schemas.ComputerTypeParams.self, request)
      return (.type(call.text), call.window, call.foreground ?? false)
    case "computer.key":
      let call = try params(Components.Schemas.ComputerKeyParams.self, request)
      guard let combo = KeyCombo(call.keys) else {
        throw Failure.badParams("\(call.keys) is not a key this Mac knows.")
      }
      return (.key(combo, repeat: call._repeat ?? 1), call.window, call.foreground ?? false)
    default:
      throw Failure.unknownMethod(method)
    }
  }

  private func params<T: Decodable>(_ type: T.Type, _ request: Data) throws -> T {
    do {
      return try decoder.decode(Call<T>.self, from: request).params
    } catch {
      throw Failure.badParams("The call's params do not fit \(type).")
    }
  }
}

private struct Call<Params: Decodable>: Decodable {
  let params: Params
}

extension Modifiers {
  fileprivate init(names: [String]?) {
    let known: [String: Modifiers] = [
      "cmd": .command, "shift": .shift, "option": .option, "ctrl": .control,
    ]
    self = Modifiers((names ?? []).compactMap { known[$0] })
  }
}

extension Components.Schemas.ComputerPoint {
  fileprivate var point: CGPoint { CGPoint(x: x, y: y) }
}

extension Components.Schemas.ComputerViewParams.RegionPayload {
  fileprivate var rect: CGRect { CGRect(x: x, y: y, width: width, height: height) }
}

extension Components.Schemas.ComputerOpenParams {
  fileprivate func target() throws -> OpenTarget {
    if let app { return .app(app) }
    if let path { return .file(path) }
    if let url, let parsed = URL(string: url) { return .url(parsed) }
    throw ComputerUse.Failure.badParams("Give an app, a path, or a URL to open.")
  }
}

extension WindowSummary {
  fileprivate var schema: Components.Schemas.ComputerWindow {
    .init(ref: ref, app: app, title: title, isFront: isFront, isMinimized: isMinimized)
  }
}

extension View {
  fileprivate var schema: Components.Schemas.ComputerView {
    .init(
      window: window.schema,
      image: .init(data: jpeg.base64EncodedString(), width: width, height: height))
  }
}

extension Outcome {
  fileprivate var schema: Components.Schemas.ComputerOutcome {
    switch result {
    case .done(let delivery):
      .init(
        delivery: .init(value1: delivery == .foreground ? .foreground : .background),
        view: view?.schema)
    case .notDone(let reason):
      .init(notDone: .init(value1: reason.schema), view: view?.schema)
    }
  }
}

extension Outcome.Reason {
  fileprivate var schema: Components.Schemas.ComputerOutcome.NotDonePayload.Value1Payload {
    switch self {
    case .needsForeground: .needsForeground
    case .declined: .declined
    case .minimized: .minimized
    }
  }
}
