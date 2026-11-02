import GoatAPI
import SwiftUI

/// Finds an agent by name, from the keyboard. Before anything is typed it lists the most recent
/// agents, so the arrows and Return alone switch between them.
struct SearchPanel: View {
  let agents: [Components.Schemas.Agent]
  let open: (Components.Schemas.Agent) -> Void
  let close: () -> Void

  @State private var query = ""
  @State private var selection = 0
  @FocusState private var isFocused: Bool

  private var matches: [Components.Schemas.Agent] {
    let query = query.trimmingCharacters(in: .whitespaces)
    let found = query.isEmpty ? agents : agents.filter { $0.name.localizedStandardContains(query) }
    return Array(found.prefix(6))
  }

  var body: some View {
    VStack(spacing: 0) {
      HStack(spacing: 10) {
        Glyph(kind: .search, size: 18)
          .foregroundStyle(Palette.mutedForeground)
        TextField("Find an Agent", text: $query)
          .textFieldStyle(.plain)
          .font(.system(size: 15))
          .focused($isFocused)
          .onSubmit(openSelection)
          .onKeyPress(.downArrow) { move(by: 1) }
          .onKeyPress(.upArrow) { move(by: -1) }
          .onExitCommand(perform: close)
      }
      .padding(.horizontal, 12)
      .frame(height: 44)
      if !matches.isEmpty {
        Palette.border.frame(height: 1)
          .padding(.horizontal, 6)
          .padding(.top, 2)
          .padding(.bottom, 6)
        VStack(spacing: 2) {
          ForEach(Array(matches.enumerated()), id: \.element.id) { index, agent in
            SidebarRow(
              agent: agent, isSelected: index == selection, showsReturn: index == selection
            )
            .onTapGesture { open(agent) }
            .onHover { if $0 { selection = index } }
          }
        }
      }
    }
    .padding(6)
    .frame(width: 480)
    .background(Palette.popover, in: .rect(cornerRadius: 18, style: .continuous))
    .overlay {
      RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(
        Palette.border, lineWidth: 0.5)
    }
    .shadow(color: .black.opacity(0.2), radius: 24, y: 12)
    .onAppear { isFocused = true }
    .onChange(of: query) { selection = 0 }
  }

  private func openSelection() {
    if matches.indices.contains(selection) { open(matches[selection]) }
  }

  private func move(by step: Int) -> KeyPress.Result {
    guard !matches.isEmpty else { return .ignored }
    selection = (selection + step + matches.count) % matches.count
    return .handled
  }
}
