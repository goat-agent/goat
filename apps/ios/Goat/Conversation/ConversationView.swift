import AuthenticationServices
import GoatAPI
import SwiftUI

struct ConversationView: View {
  @State private var conversation: ConversationModel
  @State private var integrations: IntegrationsModel
  @State private var position = ScrollPosition(edge: .bottom)
  @State private var isAtBottom = true
  @Environment(\.webAuthenticationSession) private var webAuthenticationSession

  init(agent: Components.Schemas.Agent, userName: String, client: Client) {
    _conversation = State(
      initialValue: ConversationModel(agent: agent, userName: userName, client: client)
    )
    _integrations = State(initialValue: IntegrationsModel(client: client))
  }

  var body: some View {
    ScrollView {
      LazyVStack(spacing: 6) {
        MessageBubble(text: conversation.greeting, isMine: false)
        ForEach(Array(conversation.messages.enumerated()), id: \.element.id) { index, message in
          if let time = gapBefore(index) {
            Text(time, format: .dateTime.month().day().hour().minute())
              .font(.caption)
              .foregroundStyle(.secondary)
              .padding(.top, 12)
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
          OutgoingBubble(message: message) {
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
      .padding(.horizontal)
      .padding(.vertical, 8)
    }
    .scrollPosition($position)
    .defaultScrollAnchor(.bottom)
    .scrollDismissesKeyboard(.interactively)
    .onScrollGeometryChange(for: Bool.self) { geometry in
      geometry.visibleRect.maxY >= geometry.contentSize.height - 40
    } action: { _, atBottom in
      isAtBottom = atBottom
    }
    .onChange(of: conversation.messages.count) {
      if isAtBottom { scrollToLatest() }
      if conversation.messages.last?.role.value1 == .assistant {
        AccessibilityNotification.Announcement(String(localized: "New reply")).post()
      }
    }
    .onChange(of: conversation.outgoing.count) { scrollToLatest() }
    .onChange(of: conversation.isRunning) {
      if isAtBottom { scrollToLatest() }
    }
    .overlay(alignment: .bottomTrailing) {
      if !isAtBottom {
        Button("Jump to Latest", systemImage: "arrow.down", action: scrollToLatest)
          .labelStyle(.iconOnly)
          .buttonStyle(.glass)
          .buttonBorderShape(.circle)
          .padding()
          .transition(.scale.combined(with: .opacity))
      }
    }
    .animation(.default, value: isAtBottom)
    .safeAreaBar(edge: .bottom) { Composer(draft: $conversation.draft, send: send) }
    .navigationTitle(conversation.agent.name)
    .navigationBarTitleDisplayMode(.inline)
    .task { await integrations.load() }
    .task { await conversation.listen() }
  }

  private func send() {
    Task { await conversation.send() }
  }

  private func connect(_ integrationID: String) {
    let connector = Connector(client: conversation.client, session: webAuthenticationSession)
    Task {
      await integrations.connect(integrationID, agentID: conversation.agent.id, with: connector)
    }
  }

  private func scrollToLatest() {
    withAnimation { position.scrollTo(edge: .bottom) }
  }

  /// The time to show above a message that starts a new stretch of the conversation.
  private func gapBefore(_ index: Int) -> Date? {
    let message = conversation.messages[index]
    guard index > 0 else { return message.createdAt }
    let previous = conversation.messages[index - 1]
    return message.createdAt.timeIntervalSince(previous.createdAt) > 60 * 60
      ? message.createdAt : nil
  }
}

private struct Composer: View {
  @Binding var draft: String
  let send: () -> Void

  private var canSend: Bool { !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

  var body: some View {
    GlassEffectContainer {
      HStack(alignment: .bottom, spacing: 8) {
        TextField("Message", text: $draft, axis: .vertical)
          .lineLimit(1...5)
          .padding(.horizontal, 16)
          .padding(.vertical, 11)
          .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 22))
        Button("Send", systemImage: "arrow.up", action: send)
          .labelStyle(.iconOnly)
          .font(.body.weight(.semibold))
          .buttonStyle(.glassProminent)
          .buttonBorderShape(.circle)
          .controlSize(.large)
          .disabled(!canSend)
      }
    }
    .padding(.horizontal)
    .padding(.bottom, 8)
  }
}
