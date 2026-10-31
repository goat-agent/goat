import GoatAPI
import SwiftUI

/// The sidebar's contents: search and new agent beside the window's buttons, the agents, and
/// the account at the foot. As a rail it keeps only faces and icons.
struct SidebarPanel: View {
  @Bindable var model: AppModel
  let isRail: Bool

  @State private var isSearching = false
  @State private var query = ""
  @State private var isConfirmingSignOut = false
  @FocusState private var isSearchFocused: Bool
  @Environment(\.openSettings) private var openSettings

  private var agents: [Components.Schemas.Agent] {
    let query = query.trimmingCharacters(in: .whitespaces)
    guard isSearching, !query.isEmpty else { return model.agents }
    return model.agents.filter { $0.name.localizedStandardContains(query) }
  }

  var body: some View {
    VStack(spacing: 0) {
      header
      if isSearching && !isRail {
        TextField("Search", text: $query)
          .textFieldStyle(.roundedBorder)
          .focused($isSearchFocused)
          .onSubmit { agents.first.map(open) }
          .onExitCommand(perform: endSearch)
          .padding(.horizontal, 10)
          .padding(.bottom, 8)
      }
      ScrollView {
        VStack(spacing: 2) {
          ForEach(agents, id: \.id) { agent in
            SidebarRow(
              agent: agent, isSelected: agent.id == model.agentPath.last, isRail: isRail
            ) { open(agent) }
          }
        }
        .padding(.horizontal, 8)
      }
      .scrollIndicators(.never)
      footer
    }
    .labelStyle(.iconOnly)
    .confirmationDialog("Sign out?", isPresented: $isConfirmingSignOut) {
      Button("Sign Out", role: .destructive) { Task { await model.signOut() } }
    } message: {
      Text("To sign back in, you’ll need a code from your email.")
    }
  }

  /// The window's three buttons sit at the left of this row.
  private var header: some View {
    HStack(spacing: 2) {
      Spacer()
      if !isRail {
        searchButton
        newAgentButton
      }
    }
    .padding(.horizontal, 10)
    .frame(height: 52)
  }

  @ViewBuilder private var footer: some View {
    if isRail {
      VStack(spacing: 6) {
        searchButton
          .popover(isPresented: $isSearching) {
            TextField("Search", text: $query)
              .textFieldStyle(.roundedBorder)
              .frame(width: 220)
              .padding(10)
              .onSubmit {
                agents.first.map(open)
                endSearch()
              }
          }
        newAgentButton
        accountMenu.padding(.top, 6)
      }
      .padding(.bottom, 14)
    } else {
      HStack {
        accountMenu
        Spacer()
      }
      .padding(.horizontal, 14)
      .padding(.bottom, 14)
    }
  }

  private var searchButton: some View {
    SidebarButton(title: "Search", systemImage: "magnifyingglass") {
      if isSearching {
        endSearch()
      } else {
        isSearching = true
        isSearchFocused = true
      }
    }
    .keyboardShortcut("f")
  }

  private var newAgentButton: some View {
    SidebarButton(title: "New Agent", systemImage: "plus") {
      model.isCreatingAgent = true
    }
    .keyboardShortcut("n")
  }

  private var accountMenu: some View {
    Menu {
      Button("Settings…") { openSettings() }
        .keyboardShortcut(",")
      Divider()
      Button("Sign Out…") { isConfirmingSignOut = true }
    } label: {
      AvatarCircle(name: accountName, image: nil, size: 28)
    }
    .menuStyle(.button)
    .buttonStyle(.plain)
    .menuIndicator(.hidden)
    .fixedSize()
    .accessibilityLabel("Account")
  }

  /// The user's name, or their email while they have not given one.
  private var accountName: String {
    let name = model.userName.trimmingCharacters(in: .whitespaces)
    return name.isEmpty ? model.me?.email ?? "" : name
  }

  private func open(_ agent: Components.Schemas.Agent) {
    model.agentPath = [agent.id]
    model.open(agent.id)
  }

  private func endSearch() {
    isSearching = false
    query = ""
  }
}

/// An agent in the sidebar: the full row when there is room, its face with the working dots on
/// its corner in the rail.
private struct SidebarRow: View {
  let agent: Components.Schemas.Agent
  let isSelected: Bool
  let isRail: Bool
  let select: () -> Void

  @State private var isHovered = false

  var body: some View {
    Button(action: select) {
      Group {
        if isRail {
          AvatarView(agent: agent, size: 36)
            .overlay(alignment: .bottomTrailing) {
              if agent.status.value1 == .running { WorkingBadge() }
            }
        } else {
          AgentRow(agent: agent, avatarSize: 36)
        }
      }
      .padding(.horizontal, isRail ? 0 : 8)
      .padding(.vertical, 6)
      .frame(maxWidth: .infinity, minHeight: 46)
      .background(highlight, in: .rect(cornerRadius: 8))
      .contentShape(.rect)
    }
    .buttonStyle(.plain)
    .onHover { isHovered = $0 }
    .help(isRail ? agent.name : "")
    .accessibilityLabel(agent.name)
    .accessibilityAddTraits(isSelected ? .isSelected : [])
  }

  private var highlight: AnyShapeStyle {
    if isSelected { return AnyShapeStyle(.fill.secondary) }
    return isHovered ? AnyShapeStyle(.fill.quaternary) : AnyShapeStyle(.clear)
  }
}

/// Three dots on a face's corner while the agent works.
private struct WorkingBadge: View {
  var body: some View {
    Image(systemName: "ellipsis")
      .font(.caption2.weight(.bold))
      .symbolEffect(.variableColor.iterative.dimInactiveLayers)
      .padding(.horizontal, 4)
      .padding(.vertical, 3)
      .background(.regularMaterial, in: .capsule)
      .offset(x: 4, y: 2)
      .accessibilityLabel("Working")
  }
}

private struct SidebarButton: View {
  let title: LocalizedStringKey
  let systemImage: String
  let action: () -> Void

  @State private var isHovered = false

  var body: some View {
    Button(action: action) {
      Image(systemName: systemImage)
        .font(.system(size: 15))
        .foregroundStyle(.secondary)
        .frame(width: 30, height: 28)
        .background(
          isHovered ? AnyShapeStyle(.fill.tertiary) : AnyShapeStyle(.clear),
          in: .rect(cornerRadius: 7)
        )
        .contentShape(.rect)
    }
    .buttonStyle(.plain)
    .onHover { isHovered = $0 }
    .help(Text(title))
    .accessibilityLabel(Text(title))
  }
}
