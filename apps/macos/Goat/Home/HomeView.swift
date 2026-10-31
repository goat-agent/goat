import GoatAPI
import SwiftUI

/// An inset sidebar, as shadcn/ui draws it: the window is the sidebar's color, the sidebar sits
/// on it with no panel of its own, and the conversation is a raised card inset from the window's
/// edges, with the agent's settings beside it when asked for. Drag the card's left edge to resize
/// the sidebar; narrow enough, it becomes a column of faces.
struct HomeView: View {
  @Bindable var model: AppModel

  @AppStorage("sidebarWidth") private var sidebarWidth = 248.0
  @State private var widthAtDragStart: Double?
  @State private var isShowingAgent = false

  /// Wide enough for the window's three buttons, which sit on the sidebar.
  private static let railWidth = 78.0
  private static let widthRange = 78.0...360.0
  private static let railBelow = 140.0

  private var isRail: Bool { sidebarWidth < Self.railBelow }

  /// The agent panel's width. The window is the content's size, so opening the panel widens the
  /// window by this much instead of squeezing the conversation.
  private static let panelWidth = 300.0

  var body: some View {
    HStack(spacing: 0) {
      SidebarPanel(model: model, isRail: isRail)
        .frame(width: isRail ? Self.railWidth : sidebarWidth)
      content
        .background(Color.card, in: .rect(cornerRadius: 12))
        .clipShape(.rect(cornerRadius: 12))
        .shadow(color: .black.opacity(0.08), radius: 1, y: 1)
        .shadow(color: .black.opacity(0.06), radius: 6, y: 2)
        .overlay(alignment: .leading) { resizeHandle }
        .padding([.top, .trailing, .bottom], 8)
    }
    .background(Color.sidebar)
    .ignoresSafeArea(.container, edges: .top)
    .frame(
      minWidth: 720 + (isShowingAgent ? Self.panelWidth : 0), idealWidth: 960, minHeight: 480,
      idealHeight: 640
    )
    .sheet(isPresented: $model.isCreatingAgent) {
      NewAgentSheet(model: model)
    }
  }

  @ViewBuilder private var content: some View {
    if let agent = model.agentPath.last.flatMap(model.agent) {
      HStack(spacing: 0) {
        ConversationView(model: model, agent: agent) { isShowingAgent.toggle() }
          .id(agent.id)
        if isShowingAgent {
          Divider()
          AgentPanel(model: model, agent: agent) { isShowingAgent = false }
            .frame(width: Self.panelWidth)
            .id(agent.id)
        }
      }
    } else {
      ContentUnavailableView("No Agent", systemImage: "bubble.left.and.bubble.right")
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

extension Color {
  /// The window behind the card, a step away from the card toward the middle gray.
  static let sidebar = adaptive(light: 0.96, dark: 0.15)
  /// The conversation's card.
  static let card = adaptive(light: 1, dark: 0.1)

  private static func adaptive(light: CGFloat, dark: CGFloat) -> Color {
    Color(
      nsColor: NSColor(name: nil) { appearance in
        NSColor(
          white: appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light,
          alpha: 1)
      })
  }
}
