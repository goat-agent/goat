import Foundation
import GoatAPI
import Observation

@Observable
final class ConversationModel {
  struct Outgoing: Identifiable {
    enum State {
      case sending
      /// Accepted by the server; the agent takes it at its next step.
      case queued
      case failed
    }

    let id = UUID()
    let text: String
    var state: State
  }

  let agent: Components.Schemas.Agent
  private(set) var messages: [Components.Schemas.Message] = []
  private(set) var outgoing: [Outgoing] = []
  private(set) var isRunning = false
  var draft = ""

  private let client: Client

  init(agent: Components.Schemas.Agent, client: Client) {
    self.agent = agent
    self.client = client
  }

  func listen() async {
    var delay = Duration.seconds(1)
    while !Task.isCancelled {
      do {
        try await streamEvents()
        delay = .seconds(1)
      } catch is CancellationError {
        return
      } catch {
        delay = min(delay * 2, .seconds(30))
      }
      try? await Task.sleep(for: delay)
    }
  }

  func send() async {
    let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !text.isEmpty else { return }
    draft = ""
    let message = Outgoing(text: text, state: .sending)
    outgoing.append(message)
    await deliver(message.id)
  }

  func retry(_ id: Outgoing.ID) async {
    await deliver(id)
  }

  func discard(_ id: Outgoing.ID) {
    outgoing.removeAll { $0.id == id }
  }

  private func deliver(_ id: Outgoing.ID) async {
    guard let index = outgoing.firstIndex(where: { $0.id == id }) else { return }
    outgoing[index].state = .sending
    let text = outgoing[index].text

    let accepted: Bool
    do {
      let output = try await client.sendMessage(
        path: .init(id: agent.id),
        body: .json(.init(text: text))
      )
      if case .accepted = output { accepted = true } else { accepted = false }
    } catch {
      accepted = false
    }
    if let index = outgoing.firstIndex(where: { $0.id == id }) {
      outgoing[index].state = accepted ? .queued : .failed
    }
  }

  private func streamEvents() async throws {
    let output = try await client.streamEvents(
      path: .init(id: agent.id),
      headers: .init(lastEventId: messages.last.map { String($0.id) })
    )
    for try await event in try output.ok.body.textEventStream.agentEvents() {
      apply(event)
    }
  }

  private func apply(_ event: Components.Schemas.AgentEvent) {
    switch event._type.value1 {
    case .message:
      if let message = event.message { append(message) }
    case .status:
      isRunning = event.status?.value1 == .running
    case nil:
      break
    }
  }

  private func append(_ message: Components.Schemas.Message) {
    guard message.id > messages.last?.id ?? 0 else { return }
    messages.append(message)
    if message.role.value1 == .user,
      let index = outgoing.firstIndex(where: { $0.text == message.text && $0.state != .failed })
    {
      outgoing.remove(at: index)
    }
  }
}
