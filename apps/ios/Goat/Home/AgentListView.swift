import GoatAPI
import SwiftUI

struct AgentListView: View {
  @Bindable var model: AppModel
  @State private var isShowingSettings = false

  var body: some View {
    NavigationStack(path: $model.agentPath) {
      Group {
        if model.agents.isEmpty {
          VStack(spacing: 16) {
            Text("No Agents")
              .font(.headline)
              .foregroundStyle(Palette.foreground)
            Button("New Agent") { model.isCreatingAgent = true }
              .buttonStyle(.primary)
          }
          .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
          List(model.agents, id: \.id) { agent in
            NavigationLink(value: agent.id) {
              AgentRow(agent: agent)
            }
            .navigationLinkIndicatorVisibility(.hidden)
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
          }
          .listStyle(.plain)
          .scrollContentBackground(.hidden)
        }
      }
      .background(Palette.background)
      .navigationTitle("Agents")
      .toolbar {
        ToolbarItem(placement: .topBarLeading) {
          Button("Settings", systemImage: "gearshape") { isShowingSettings = true }
        }
        ToolbarItem(placement: .topBarTrailing) {
          Button("New Agent", systemImage: "plus") { model.isCreatingAgent = true }
        }
      }
      .safeAreaBar(edge: .bottom) {
        if !model.isOnline {
          ConnectingBanner()
            .padding(.bottom, 8)
        }
      }
      .navigationDestination(for: String.self) { agentID in
        if let agent = model.agent(agentID) {
          ConversationView(model: model, agent: agent)
            .onAppear { model.open(agentID) }
        }
      }
    }
    .task { await model.listenToAgents() }
    .sheet(isPresented: $isShowingSettings) {
      SettingsView(model: model)
    }
    .sheet(isPresented: $model.isCreatingAgent) {
      NewAgentSheet(model: model)
    }
  }
}
