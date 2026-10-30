import AuthenticationServices
import GoatAPI
import QuickLook
import SwiftUI
import UniformTypeIdentifiers

/// The conversation as the Mac shows it: a quiet title row, messages in a column no wider than
/// reading is comfortable, and the composer floating at the bottom of the card.
struct ConversationView: View {
  @State private var conversation: ConversationModel
  @State private var integrations: IntegrationsModel
  @State private var isPickingFiles = false
  @State private var previewURL: URL?
  @Environment(\.webAuthenticationSession) private var webAuthenticationSession

  private static let columnWidth: CGFloat = 720

  init(agent: Components.Schemas.Agent, userName: String, client: Client) {
    _conversation = State(
      initialValue: ConversationModel(agent: agent, userName: userName, client: client))
    _integrations = State(initialValue: IntegrationsModel(client: client))
  }

  var body: some View {
    VStack(spacing: 0) {
      header
      messages
    }
    .safeAreaBar(edge: .bottom) {
      Composer(conversation: conversation) { isPickingFiles = true }
        .frame(maxWidth: Self.columnWidth)
        .padding(.horizontal, 24)
        .padding(.bottom, 16)
    }
    .fileImporter(
      isPresented: $isPickingFiles,
      allowedContentTypes: [.image, .pdf, .plainText, .commaSeparatedText],
      allowsMultipleSelection: true,
      onCompletion: attach
    )
    .quickLookPreview($previewURL)
    .task { await integrations.load() }
    .task { await conversation.listen() }
  }

  private var header: some View {
    HStack(spacing: 8) {
      AgentAvatar(agentID: conversation.agent.id, size: 22)
      Text(conversation.agent.name)
        .font(.headline)
    }
    .frame(maxWidth: .infinity)
    .frame(height: 44)
  }

  private var messages: some View {
    ScrollView {
      LazyVStack(spacing: 8) {
        MessageBubble(text: conversation.greeting, isMine: false)
        ForEach(conversation.messages, id: \.id) { message in
          ForEach(message.files ?? [], id: \.id) { file in
            SentFile(file: file, load: conversation.localURL(for:)) { previewURL = $0 }
          }
          if let steps = message.steps, !steps.isEmpty {
            StepList(steps: steps)
          }
          if !message.text.isEmpty {
            MessageBubble(text: message.text, isMine: message.role.value1 == .user)
          }
          ForEach(Array((message.connect ?? []).enumerated()), id: \.offset) { _, request in
            ConnectCard(
              name: request.name,
              iconURL: integrations.iconURL(of: request.integrationId),
              isConnected: integrations.isConnected(request.integrationId),
              isConnecting: integrations.connectingID == request.integrationId,
              isEnabled: integrations.connectingID == nil,
              didFail: integrations.failedIntegrationID == request.integrationId
            ) {
              connect(request.integrationId)
            }
          }
        }
        ForEach(conversation.outgoing) { message in
          OutgoingBubble(message: message, load: conversation.localURL(for:)) {
            previewURL = $0
          } retry: {
            Task { await conversation.retry(message.id) }
          } discard: {
            conversation.discard(message.id)
          }
        }
        if conversation.isRunning {
          TypingIndicator()
            .transition(.opacity)
        }
      }
      .animation(.default, value: conversation.isRunning)
      .frame(maxWidth: Self.columnWidth)
      .padding(.horizontal, 24)
      .padding(.vertical, 12)
      .frame(maxWidth: .infinity)
    }
    .defaultScrollAnchor(.bottom)
  }

  private func attach(_ result: Result<[URL], any Error>) {
    guard case .success(let urls) = result else { return }
    for url in urls {
      Task {
        guard let file = try? await PreparedFile.file(at: url) else { return }
        await conversation.attach(file)
      }
    }
  }

  private func connect(_ integrationID: String) {
    let connector = Connector(client: conversation.client, session: webAuthenticationSession)
    Task {
      await integrations.connect(integrationID, agentID: conversation.agent.id, with: connector)
    }
  }
}
