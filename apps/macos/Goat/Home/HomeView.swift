import GoatAPI
import SwiftUI

/// An inset sidebar, as shadcn/ui draws it: the window is the sidebar's color, the sidebar sits
/// on it with no panel of its own, and the conversation is a raised card inset from the window's
/// edges. The agent's settings open over the window, so they never change its size. Drag the
/// card's left edge to resize the sidebar; narrow enough, it becomes a column of faces.
struct HomeView: View {
  @Bindable var model: AppModel

  @AppStorage("sidebarWidth") private var sidebarWidth = 248.0
  @State private var widthAtDragStart: Double?
  @State private var isShowingAgent = false
  @State private var isSearching = false
  @State private var isConfirmingSignOut = false

  /// Equal margins either side of the window's buttons, which end 68.5 pt in.
  private static let railWidth = 78.0
  private static let widthRange = 78.0...360.0
  private static let railBelow = 140.0

  private var isRail: Bool { sidebarWidth < Self.railBelow }

  var body: some View {
    HStack(spacing: 0) {
      SidebarPanel(model: model, isRail: isRail) {
        isSearching = true
      } signOut: {
        isConfirmingSignOut = true
      }
      .frame(width: isRail ? Self.railWidth : sidebarWidth)
      content
        .background(Palette.background, in: .rect(cornerRadius: 12, style: .continuous))
        .clipShape(.rect(cornerRadius: 12, style: .continuous))
        .shadow(color: .black.opacity(0.06), radius: 1, y: 1)
        .shadow(color: .black.opacity(0.05), radius: 6, y: 2)
        .overlay(alignment: .leading) { resizeHandle }
        .padding([.top, .trailing, .bottom], 8)
    }
    .background(Palette.sidebar)
    .ignoresSafeArea(.container, edges: .top)
    .portalDialog(isPresented: $isSearching) {
      SearchPanel(agents: model.agents) { agent in
        isSearching = false
        model.show(agent.id)
      } close: {
        isSearching = false
      }
      .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
      .padding(.top, 56)
    }
    .portalDialog(isPresented: $isShowingAgent) {
      if let agent = model.agentPath.last.flatMap(model.agent) {
        AgentSettingsDialog(model: model, agent: agent) { isShowingAgent = false }
          .id(agent.id)
      }
    }
    .alertDialog(
      "Sign out?", isPresented: $isConfirmingSignOut,
      message: "To sign back in, you’ll need a code from your email.", action: "Sign Out"
    ) {
      Task { await model.signOut() }
    }
    // Search, the agent's settings, and confirmations all open here, over the whole window.
    .portalHost()
    .frame(minWidth: 720, idealWidth: 960, minHeight: 480, idealHeight: 640)
    .sheet(isPresented: $model.isCreatingAgent) {
      NewAgentSheet(model: model)
    }
  }

  @ViewBuilder private var content: some View {
    if let agent = model.agentPath.last.flatMap(model.agent) {
      ConversationView(model: model, agent: agent) { isShowingAgent = true }
        .id(agent.id)
    } else {
      Text("No Agent")
        .foregroundStyle(Palette.mutedForeground)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
  }

  private var resizeHandle: some View {
    Color.clear
      .frame(width: 8)
      .contentShape(.rect)
      .offset(x: -4)
      .pointerStyle(.frameResize(position: .trailing))
      .gesture(
        DragGesture(minimumDistance: 1, coordinateSpace: .global)
          .onChanged { value in
            let start = widthAtDragStart ?? (isRail ? Self.railWidth : sidebarWidth)
            widthAtDragStart = start
            let width = start + value.translation.width
            sidebarWidth = min(max(width, Self.widthRange.lowerBound), Self.widthRange.upperBound)
          }
          .onEnded { _ in
            widthAtDragStart = nil
            if isRail { sidebarWidth = Self.railWidth }
          }
      )
  }
}

extension AppModel {
  func show(_ agentID: String) {
    agentPath = [agentID]
    open(agentID)
  }
}
