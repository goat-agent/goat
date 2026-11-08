import Foundation

/// Keeps this Mac's WebSocket to the server open and answers each JSON-RPC call on it.
@Observable
final class DeviceConnection {
  enum State {
    case offline, connecting, online
  }

  /// Answers a call given its method and the whole request, returning the result as JSON.
  typealias Answer = (_ method: String, _ request: Data) async throws -> Data

  private(set) var state = State.offline
  private var loop: Task<Void, Never>?

  func start(token: String, answer: @escaping Answer, onRejected: @escaping () -> Void) {
    stop()
    loop = Task {
      var delay = Duration.seconds(1)
      while !Task.isCancelled {
        state = .connecting
        let socket = URLSession.shared.webSocketTask(with: Self.request(token: token))
        socket.resume()
        do {
          try await socket.ping()
          state = .online
          delay = .seconds(1)
          while !Task.isCancelled {
            let request = try await socket.receive().data
            Task { await Self.reply(to: request, on: socket, answer: answer) }
          }
        } catch {
          socket.cancel(with: .goingAway, reason: nil)
        }
        state = .offline
        if (socket.response as? HTTPURLResponse)?.statusCode == 401 {
          onRejected()
          return
        }
        try? await Task.sleep(for: delay)
        delay = min(delay * 2, .seconds(30))
      }
    }
  }

  func stop() {
    loop?.cancel()
    loop = nil
    state = .offline
  }

  private static func request(token: String) -> URLRequest {
    var components =
      URLComponents(url: .goatServer, resolvingAgainstBaseURL: false) ?? URLComponents()
    let isSecure = components.scheme == "https"
    components.scheme = isSecure ? "wss" : "ws"
    components.path = "/devices/connect"
    var request = URLRequest(url: components.url ?? .goatServer)
    request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
    return request
  }

  private static func reply(
    to request: Data, on socket: URLSessionWebSocketTask, answer: Answer
  ) async {
    guard let call = try? JSONDecoder().decode(Call.self, from: request) else { return }
    let response: Data
    do {
      let result = try await answer(call.method, request)
      response = try Response.success(id: call.id, result: result)
    } catch {
      response = (try? Response.failure(id: call.id, message: error.localizedDescription)) ?? Data()
    }
    try? await socket.send(.string(String(decoding: response, as: UTF8.self)))
  }
}

private struct Call: Decodable {
  let id: String
  let method: String
}

private enum Response {
  static func success(id: String, result: Data) throws -> Data {
    let object = try JSONSerialization.jsonObject(with: result, options: .fragmentsAllowed)
    return try JSONSerialization.data(withJSONObject: [
      "jsonrpc": "2.0", "id": id, "result": object,
    ])
  }

  static func failure(id: String, message: String) throws -> Data {
    try JSONSerialization.data(withJSONObject: [
      "jsonrpc": "2.0", "id": id, "error": ["code": -32000, "message": message],
    ])
  }
}

extension URLSessionWebSocketTask {
  fileprivate func ping() async throws {
    try await withCheckedThrowingContinuation {
      (continuation: CheckedContinuation<Void, any Error>) in
      sendPing { error in
        if let error { continuation.resume(throwing: error) } else { continuation.resume() }
      }
    }
  }
}

extension URLSessionWebSocketTask.Message {
  fileprivate var data: Data {
    switch self {
    case .string(let text): Data(text.utf8)
    case .data(let data): data
    @unknown default: Data()
    }
  }
}
