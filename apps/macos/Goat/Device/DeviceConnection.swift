import Foundation
import Observation

/// Keeps this Mac's WebSocket to the server open while someone is signed in, and hands each
/// request that arrives to the computer service. The server never queues calls for a closed
/// socket, so a drop only costs the calls in flight.
@Observable
final class DeviceConnection {
  private(set) var isConnected = false

  @ObservationIgnored private var task: Task<Void, Never>?

  func start(serverURL: URL, token: @escaping () -> String?, service: ComputerService) {
    guard task == nil else { return }
    task = Task { await run(serverURL: serverURL, token: token, service: service) }
  }

  func stop() {
    task?.cancel()
    task = nil
    isConnected = false
  }

  private func run(serverURL: URL, token: () -> String?, service: ComputerService) async {
    var delay = Duration.seconds(1)
    while !Task.isCancelled {
      if let token = token() {
        do {
          try await connect(to: socketURL(serverURL), token: token, service: service)
          delay = .seconds(1)
        } catch {
          delay = min(delay * 2, .seconds(30))
        }
      }
      isConnected = false
      try? await Task.sleep(for: delay)
    }
  }

  private func connect(to url: URL, token: String, service: ComputerService) async throws {
    var request = URLRequest(url: url)
    request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
    let socket = URLSession.shared.webSocketTask(with: request)
    socket.resume()
    defer { socket.cancel(with: .goingAway, reason: nil) }

    try await withThrowingTaskGroup { group in
      while !Task.isCancelled {
        let message = try await socket.receive()
        isConnected = true
        let data: Data? =
          switch message {
          case .string(let text): Data(text.utf8)
          case .data(let data): data
          @unknown default: nil
          }
        guard let data else { continue }
        // Each call runs on its own, so a long one does not hold up a status check.
        group.addTask {
          guard let answer = await service.answer(data),
            let text = String(data: answer, encoding: .utf8)
          else { return }
          try await socket.send(.string(text))
        }
      }
    }
  }

  private func socketURL(_ serverURL: URL) -> URL {
    var components = URLComponents(
      url: serverURL.appending(path: "devices/connect"), resolvingAgainstBaseURL: false)
    components?.scheme = serverURL.scheme == "https" ? "wss" : "ws"
    return components?.url ?? serverURL
  }
}
