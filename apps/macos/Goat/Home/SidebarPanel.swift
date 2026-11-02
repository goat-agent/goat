import GoatAPI
import SwiftUI

/// The sidebar's contents: search and new agent on the line of the window's buttons, the agents,
/// and the account at the foot. As a rail it keeps the faces where they were and moves the two
/// buttons above the account.
struct SidebarPanel: View {
  let model: AppModel
  let isRail: Bool
  let search: () -> Void
  let signOut: () -> Void

  var body: some View {
    VStack(spacing: 0) {
      HStack(spacing: 2) {
        Spacer()
        if !isRail { buttons }
      }
      .padding(.horizontal, 8)
      .frame(height: 32)
      // The list starts where the title bar ends, so the scroll view reaches up behind the
      // buttons, as scroll views do at the safe area's edge. They must sit above it to be clicked.
      .zIndex(1)
      ScrollView {
        VStack(spacing: 2) {
          ForEach(model.agents, id: \.id) { agent in
            Button {
              model.show(agent.id)
            } label: {
              SidebarRow(agent: agent, isSelected: agent.id == model.agentPath.last, isRail: isRail)
            }
            .buttonStyle(.plain)
            .help(isRail ? agent.name : "")
          }
        }
        .padding(.horizontal, isRail ? 13 : 8)
        .padding(.top, 8)
      }
      .scrollIndicators(.never)
      footer
    }
  }

  @ViewBuilder private var footer: some View {
    if isRail {
      VStack(spacing: 4) {
        buttons
        accountButton
          .padding(.top, 6)
      }
      .padding(.vertical, 10)
    } else {
      accountButton
        .padding(.leading, 18)
        .frame(height: 52)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
  }

  @ViewBuilder private var buttons: some View {
    Button(action: search) {
      Label {
        Text("Search")
      } icon: {
        Glyph(kind: .search)
      }
    }
    .buttonStyle(.icon)
    .keyboardShortcut("k")
    Button {
      model.isCreatingAgent = true
    } label: {
      Label {
        Text("New Agent")
      } icon: {
        Glyph(kind: .plus)
      }
    }
    .buttonStyle(.icon)
    .keyboardShortcut("n")
  }

  private var accountButton: some View {
    AccountButton(name: model.userName, email: model.me?.email ?? "", signOut: signOut)
  }
}

/// An agent as the sidebar and search show it: face, name, and the last message or the working
/// dots. In the rail, only the face, with the dots on its corner.
struct SidebarRow: View {
  let agent: Components.Schemas.Agent
  let isSelected: Bool
  var isRail = false
  var showsReturn = false

  @State private var isHovered = false

  private var isWorking: Bool { agent.status.value1 == .running }

  var body: some View {
    HStack(spacing: 10) {
      AvatarView(agent: agent, size: 32)
        .overlay(alignment: .bottomTrailing) {
          if isRail && isWorking { WorkingBadge() }
        }
      if !isRail {
        VStack(alignment: .leading, spacing: 0) {
          Text(agent.name)
            .font(.body.weight(.medium))
            .frame(height: 16)
          Group {
            if isWorking {
              WorkingDots()
            } else {
              Text(agent.preview)
            }
          }
          .font(.callout)
          .foregroundStyle(Palette.mutedForeground)
          .frame(height: 16)
        }
        .lineLimit(1)
        Spacer(minLength: 0)
        if showsReturn {
          Text(verbatim: "↩")
            .font(.callout)
            .foregroundStyle(Palette.mutedForeground)
        }
      }
    }
    .padding(10)
    .background(
      isSelected ? Palette.sidebarAccent : isHovered ? Palette.accent : .clear,
      in: .rect(cornerRadius: 12, style: .continuous)
    )
    .contentShape(.rect(cornerRadius: 12, style: .continuous))
    .onHover { isHovered = $0 }
    .animation(.easeOut(duration: 0.12), value: isHovered)
    .accessibilityElement(children: .combine)
    .accessibilityAddTraits(isSelected ? .isSelected : [])
  }
}

/// The user's face, which opens who is signed in, Settings, and Sign Out above it.
private struct AccountButton: View {
  let name: String
  let email: String
  let signOut: () -> Void

  @State private var isHovered = false
  @State private var isShowingMenu = false
  @Environment(\.openSettings) private var openSettings

  var body: some View {
    Button {
      isShowingMenu = true
    } label: {
      AvatarCircle(name: name.isEmpty ? email : name, image: nil, size: 32)
        .opacity(isShowingMenu || isHovered ? 0.8 : 1)
        .contentShape(.circle)
    }
    .buttonStyle(.plain)
    .onHover { isHovered = $0 }
    .animation(.easeOut(duration: 0.12), value: isHovered)
    .accessibilityLabel("Account")
    .dropdownMenu(isPresented: $isShowingMenu, placement: .above(.leading)) {
      DropdownMenuLabel(title: name, subtitle: email)
      DropdownMenuSeparator()
      DropdownMenuItem(title: "Settings", systemImage: "gearshape", shortcut: "⌘,") {
        openSettings()
      }
      DropdownMenuItem(
        title: "Sign Out", systemImage: "rectangle.portrait.and.arrow.right", action: signOut)
    }
  }
}

private struct WorkingDots: View {
  var body: some View {
    Image(systemName: "ellipsis")
      .fontWeight(.bold)
      .symbolEffect(.variableColor.iterative.dimInactiveLayers)
      .accessibilityLabel("Working")
  }
}

private struct WorkingBadge: View {
  var body: some View {
    WorkingDots()
      .font(.system(size: 8))
      .foregroundStyle(Palette.mutedForeground)
      .padding(.horizontal, 3)
      .frame(height: 13)
      .background(Palette.background, in: .capsule)
      .shadow(color: .black.opacity(0.15), radius: 1, y: 1)
      .offset(x: 6, y: 3)
  }
}
