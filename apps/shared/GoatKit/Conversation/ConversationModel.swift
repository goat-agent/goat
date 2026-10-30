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
    let files: [Components.Schemas.File]
    var state: State
  }

  /// A file in the composer. It uploads as soon as it is picked, so sending waits on nothing.
  struct Attachment: Identifiable {
    enum State {
      case uploading
      case uploaded(Components.Schemas.File)
      case failed
    }

    let id = UUID()
    let name: String
    var localURL: URL
    var state: State

    var file: Components.Schemas.File? {
      if case .uploaded(let file) = state { file } else { nil }
    }
  }

  /// The server takes this many files in one message.
  static let maxAttachments = 10

  let agent: Components.Schemas.Agent
  /// Drawn by the app rather than stored, so each device shows it in its own language.
  let greeting: String
  private(set) var messages: [Components.Schemas.Message] = []
  private(set) var outgoing: [Outgoing] = []
  private(set) var attachments: [Attachment] = []
  private(set) var isRunning = false
  var draft = ""

  var canAttach: Bool { attachments.count < Self.maxAttachments }

  /// Nothing typed and nothing attached, so the composer offers a call instead of sending.
  var isEmpty: Bool {
    draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && attachments.isEmpty
  }

  var canSend: Bool {
    !isEmpty && attachments.allSatisfy { $0.file != nil }
  }

  let client: Client
  let dictation: Dictation

  init(agent: Components.Schemas.Agent, userName: String, client: Client) {
    self.agent = agent
    greeting = String(localized: "Hi, \(userName)! I’m \(agent.name). How can I help?")
    self.client = client
    dictation = Dictation(agentID: agent.id, client: client)
    dictation.onText = { [weak self] text in self?.insert(text) }
  }

  /// Spoken text goes after what is already typed, so typing and speaking can mix.
  private func insert(_ text: String) {
    if draft.isEmpty || draft.last?.isWhitespace == true {
      draft += text
    } else {
      draft += " " + text
    }
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
    guard canSend else { return }
    let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
    let files = attachments.compactMap(\.file)
    draft = ""
    attachments = []
    let message = Outgoing(text: text, files: files, state: .sending)
    outgoing.append(message)
    await deliver(message.id)
  }

  func attach(_ file: PreparedFile) async {
    guard canAttach, let url = try? await FileCache.stage(file.data, name: file.name) else {
      return
    }
    let attachment = Attachment(name: file.name, localURL: url, state: .uploading)
    attachments.append(attachment)
    await upload(attachment.id)
  }

  func retryUpload(_ id: Attachment.ID) async {
    await upload(id)
  }

  func removeAttachment(_ id: Attachment.ID) {
    attachments.removeAll { $0.id == id }
  }

  /// The file on this device, downloaded first if it is not here yet.
  func localURL(for file: Components.Schemas.File) async -> URL? {
    if let cached = FileCache.existing(id: file.id, name: file.name) { return cached }
    guard let data = try? await client.fileData(agentID: agent.id, fileID: file.id) else {
      return nil
    }
    return try? await FileCache.store(data, id: file.id, name: file.name)
  }

  private func upload(_ id: Attachment.ID) async {
    guard let index = attachments.firstIndex(where: { $0.id == id }) else { return }
    attachments[index].state = .uploading
    let staged = attachments[index].localURL
    let name = attachments[index].name

    var result: (file: Components.Schemas.File, url: URL)?
    do {
      let data = try await FileCache.read(staged)
      let output = try await client.uploadFile(
        path: .init(id: agent.id),
        body: .file(data, named: name)
      )
      let file = try output.created.body.json.file
      result = (file, try await FileCache.adopt(staged, id: file.id, name: file.name))
    } catch {
      result = nil
    }
    guard let index = attachments.firstIndex(where: { $0.id == id }) else { return }
    if let result {
      attachments[index].state = .uploaded(result.file)
      attachments[index].localURL = result.url
    } else {
      attachments[index].state = .failed
    }
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
    let fileIDs = outgoing[index].files.map(\.id)

    let accepted: Bool
    do {
      let output = try await client.sendMessage(
        path: .init(id: agent.id),
        body: .json(.init(text: text.isEmpty ? nil : text, files: fileIDs.isEmpty ? nil : fileIDs))
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
    let fileIDs = (message.files ?? []).map(\.id)
    if message.role.value1 == .user,
      let index = outgoing.firstIndex(where: {
        $0.text == message.text && $0.files.map(\.id) == fileIDs && $0.state != .failed
      })
    {
      outgoing.remove(at: index)
    }
  }
}
