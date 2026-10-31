import GoatAPI
import QuickLook
import SwiftUI

/// One agent's conversation on iPhone: the agent's title at the top, the composer above the
/// keyboard, calls over the whole screen, and the agent's settings in a sheet.
struct ConversationView: View {
  let model: AppModel
  /// The agent as it is now, so a rename or a new picture shows at once.
  let agent: Components.Schemas.Agent

  @State private var conversation: ConversationModel
  @State private var integrations: IntegrationsModel
  @State private var isPickingPhotos = false
  @State private var isPickingFiles = false
  @State private var previewURL: URL?
  @State private var call: Call?
  @State private var isShowingAgent = false

  init(model: AppModel, agent: Components.Schemas.Agent) {
    self.model = model
    self.agent = agent
    _conversation = State(initialValue: ConversationModel(agentID: agent.id, client: model.client))
    _integrations = State(initialValue: IntegrationsModel(client: model.client))
  }

  var body: some View {
    ConversationMessages(
      conversation: conversation, integrations: integrations, agent: agent,
      userName: model.userName
    ) { previewURL = $0 }
    .safeAreaBar(edge: .bottom) {
      Composer(conversation: conversation) {
        isPickingPhotos = true
      } pickFiles: {
        isPickingFiles = true
      } open: {
        previewURL = $0
      } call: {
        call = Call(agentID: agent.id, client: model.client)
      }
      .padding(.horizontal)
      .padding(.bottom, 8)
    }
    .attachmentPickers(
      for: conversation, isPickingPhotos: $isPickingPhotos, isPickingFiles: $isPickingFiles
    )
    .quickLookPreview($previewURL)
    .fullScreenCover(item: $call) { call in
      CallView(
        call: call, agent: agent, conversation: conversation, integrations: integrations
      ) {
        self.call = nil
      }
    }
    .navigationTitle(agent.name)
    .navigationBarTitleDisplayMode(.inline)
    .toolbar {
      ToolbarItem(placement: .principal) {
        AgentTitle(agent: agent) { isShowingAgent = true }
      }
    }
    .sheet(isPresented: $isShowingAgent) {
      AgentSheet(model: model, agent: agent)
    }
    .task { await integrations.load() }
    .task { await conversation.listen() }
  }
}
