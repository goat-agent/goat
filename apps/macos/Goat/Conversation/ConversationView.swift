import GoatAPI
import QuickLook
import SwiftUI

/// One agent's conversation in the Mac's card: the agent's title at the top, lines no wider than
/// reading is comfortable, the composer at the bottom, and a call in the card's place.
struct ConversationView: View {
  let model: AppModel
  /// The agent as it is now, so a rename or a new picture shows at once.
  let agent: Components.Schemas.Agent
  let toggleAgent: () -> Void

  @State private var conversation: ConversationModel
  @State private var integrations: IntegrationsModel
  @State private var isPickingPhotos = false
  @State private var isPickingFiles = false
  @State private var previewURL: URL?
  @State private var call: Call?

  private static let columnWidth: CGFloat = 640

  init(model: AppModel, agent: Components.Schemas.Agent, toggleAgent: @escaping () -> Void) {
    self.model = model
    self.agent = agent
    self.toggleAgent = toggleAgent
    _conversation = State(initialValue: ConversationModel(agentID: agent.id, client: model.client))
    _integrations = State(initialValue: IntegrationsModel(client: model.client))
  }

  var body: some View {
    Group {
      if let call {
        CallView(
          call: call, agent: agent, conversation: conversation, integrations: integrations
        ) {
          self.call = nil
        }
      } else {
        conversationCard
      }
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .task { await integrations.load() }
    .task { await conversation.listen() }
  }

  /// The title, the messages, and the composer, one above the other. A top bar that the
  /// messages scroll under would join the window's title bar on macOS 26 and take the clicks
  /// meant for the sidebar's buttons.
  private var conversationCard: some View {
    VStack(spacing: 0) {
      VStack(spacing: 6) {
        AgentTitle(agent: agent, action: toggleAgent)
        if !model.isOnline {
          ConnectingBanner()
            .transition(.opacity)
        }
      }
      .padding(.top, 8)
      .padding(.bottom, 4)
      .animation(.default, value: model.isOnline)
      ConversationMessages(
        conversation: conversation, integrations: integrations, agent: agent,
        userName: model.userName, open: { previewURL = $0 }, maxWidth: Self.columnWidth
      )
      .frame(maxHeight: .infinity)
      .overlay(alignment: .top) { edgeFade(from: .top) }
      .overlay(alignment: .bottom) { edgeFade(from: .bottom) }
      Composer(conversation: conversation) {
        isPickingPhotos = true
      } pickFiles: {
        isPickingFiles = true
      } open: {
        previewURL = $0
      } call: {
        call = Call(agentID: agent.id, client: model.client)
      }
      // The same column as the messages, so its edges meet the bubbles'.
      .padding(.horizontal, 16)
      .frame(maxWidth: Self.columnWidth)
      .padding(.top, 8)
      .padding(.bottom, 16)
    }
    .attachmentPickers(
      for: conversation, isPickingPhotos: $isPickingPhotos, isPickingFiles: $isPickingFiles
    )
    .quickLookPreview($previewURL)
  }

  /// Messages fade out at the list's edges instead of being cut by them.
  private func edgeFade(from edge: UnitPoint) -> some View {
    LinearGradient(
      colors: [Palette.background, Palette.background.opacity(0)], startPoint: edge,
      endPoint: edge == .top ? .bottom : .top
    )
    .frame(height: 12)
    .allowsHitTesting(false)
  }
}
