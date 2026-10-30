import GoatAPI
import SwiftUI

struct AgentListView: View {
  @Bindable var model: AppModel
  @State private var isShowingSettings = false

  var body: some View {
    NavigationStack(path: $model.agentPath) {
      List(model.agents, id: \.id) { agent in
        NavigationLink(agent.name, value: agent)
      }
      .navigationTitle("Agents")
      .toolbar {
        ToolbarItemGroup(placement: .topBarTrailing) {
          Button("Settings", systemImage: "gearshape") { isShowingSettings = true }
          Button("New Agent", systemImage: "plus") { model.isCreatingAgent = true }
        }
      }
      .navigationDestination(for: Components.Schemas.Agent.self) { agent in
        ConversationView(agent: agent, userName: model.userName, client: model.client)
          .onAppear { model.open(agent) }
      }
    }
    .sheet(isPresented: $isShowingSettings) {
      SettingsView(model: model)
    }
    .sheet(isPresented: $model.isCreatingAgent) {
      NavigationStack { AgentSetupView(model: model) }
    }
  }
}
