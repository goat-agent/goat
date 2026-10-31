import GoatAPI
import SwiftUI

struct AgentListView: View {
  @Bindable var model: AppModel
  @State private var isShowingSettings = false

  var body: some View {
    NavigationStack(path: $model.agentPath) {
      Group {
        if model.agents.isEmpty {
          ContentUnavailableView {
            Text("No Agents")
          } actions: {
            Button("New Agent") { model.isCreatingAgent = true }
              .buttonStyle(.glassProminent)
          }
        } else {
          List(model.agents, id: \.id) { agent in
            NavigationLink(value: agent.id) {
              AgentRow(agent: agent)
            }
            .navigationLinkIndicatorVisibility(.hidden)
            .listSectionSeparator(.hidden)
          }
          .listStyle(.plain)
        }
      }
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

private struct ConnectingBanner: View {
  var body: some View {
    HStack(spacing: 10) {
      ProgressView()
      Text("Connecting…")
    }
    .font(.subheadline)
    .padding(.horizontal, 16)
    .padding(.vertical, 10)
    .glassEffect(.regular, in: .capsule)
    .padding(.bottom, 8)
  }
}
