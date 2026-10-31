import AuthenticationServices
import GoatAPI
import SwiftUI

/// The conversation itself, the same on both devices: messages oldest first, kept at the newest
/// until the user scrolls away, with a button back down when they have.
struct ConversationMessages: View {
  let conversation: ConversationModel
  let integrations: IntegrationsModel
  let agent: Components.Schemas.Agent
  let userName: String
  let open: (URL) -> Void
  /// The Mac stops lines at a readable width; an iPhone screen is narrow enough already.
  var maxWidth: CGFloat = .infinity

  @State private var isAtBottom = true
  /// Stays on the newest message until the user scrolls away.
  @State private var isFollowing = true
  @Environment(\.webAuthenticationSession) private var webAuthenticationSession

  private static let end = "end"

  var body: some View {
    // The scroll view appears only once the first page is in: its default anchor places it at
    // the end only for the content it first appears with.
    if conversation.isLoaded {
      scrollView
    } else {
      Color.clear
    }
  }

  private var scrollView: some View {
    ScrollViewReader { proxy in
      ScrollView {
        messages
          .frame(maxWidth: maxWidth)
          .frame(maxWidth: .infinity)
        Color.clear
          .frame(height: 1)
          .id(Self.end)
      }
      .defaultScrollAnchor(.bottom)
      .scrollDismissesKeyboard(.interactively)
      .onScrollGeometryChange(for: Bool.self) { geometry in
        geometry.visibleRect.maxY >= geometry.contentSize.height - 40
      } action: { _, atBottom in
        isAtBottom = atBottom
      }
      .onScrollPhaseChange { old, new in
        if new == .idle, old == .interacting || old == .decelerating { isFollowing = isAtBottom }
      }
      // The content grows with new messages, the typing dots, pictures that finish loading, and
      // text that re-wraps when the width changes; following keeps the newest in view.
      .onScrollGeometryChange(for: CGFloat.self) { geometry in
        geometry.contentSize.height
      } action: { _, _ in
        if isFollowing { proxy.scrollTo(Self.end, anchor: .bottom) }
      }
      .onChange(of: conversation.outgoing.count) {
        isFollowing = true
        proxy.scrollTo(Self.end, anchor: .bottom)
      }
      .onChange(of: conversation.messages.count) {
        if conversation.messages.last?.role.value1 == .assistant {
          AccessibilityNotification.Announcement(String(localized: "New reply")).post()
        }
      }
      .overlay(alignment: .bottomTrailing) {
        if !isAtBottom {
          Button("Jump to Latest", systemImage: "arrow.down") {
            isFollowing = true
            withAnimation { proxy.scrollTo(Self.end, anchor: .bottom) }
          }
          .labelStyle(.iconOnly)
          .buttonStyle(.glass)
          .buttonBorderShape(.circle)
          .padding()
          .transition(.scale.combined(with: .opacity))
        }
      }
      .animation(.default, value: isAtBottom)
    }
  }

  private var messages: some View {
    LazyVStack(spacing: 6) {
      MessageBubble(text: greeting, isMine: false)
      ForEach(Array(conversation.messages.enumerated()), id: \.element.id) { index, message in
        if let time = gapBefore(index) {
          Text(time, format: .dateTime.month().day().hour().minute())
            .font(.caption)
            .foregroundStyle(.secondary)
            .padding(.top, 12)
        }
        ForEach(message.files ?? [], id: \.id) { file in
          SentFile(file: file, load: conversation.localURL(for:), open: open)
        }
        if let steps = message.steps, !steps.isEmpty {
          StepList(steps: steps)
        }
        if !message.text.isEmpty {
          MessageBubble(text: message.text, isMine: message.role.value1 == .user)
        }
        ForEach(Array((message.connect ?? []).enumerated()), id: \.offset) { _, request in
          ConnectCard(
            request: request, integrations: integrations, agentID: agent.id,
            session: webAuthenticationSession)
        }
      }
      ForEach(conversation.outgoing) { message in
        OutgoingBubble(message: message, load: conversation.localURL(for:), open: open) {
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

  /// Drawn by the app rather than stored, so each device shows it in its own language.
  private var greeting: String {
    userName.isEmpty
      ? String(localized: "Hi! I’m \(agent.name). How can I help?")
      : String(localized: "Hi, \(userName)! I’m \(agent.name). How can I help?")
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
