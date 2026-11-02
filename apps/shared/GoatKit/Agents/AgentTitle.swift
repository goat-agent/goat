import GoatAPI
import SwiftUI

/// The open agent at the top of its conversation: its face and name, lit under the pointer.
/// Tapping it opens the agent's settings.
struct AgentTitle: View {
  let agent: Components.Schemas.Agent
  let action: () -> Void

  #if os(macOS)
    private let avatarSize: CGFloat = 24
    private let inset: CGFloat = 2
    private let trailing: CGFloat = 12
  #else
    private let avatarSize: CGFloat = 32
    private let inset: CGFloat = 6
    private let trailing: CGFloat = 16
  #endif

  @State private var isHovered = false

  var body: some View {
    Button(action: action) {
      HStack(spacing: 8) {
        AvatarView(agent: agent, size: avatarSize)
        Text(agent.name)
          .font(.headline)
          .foregroundStyle(Palette.foreground)
          .lineLimit(1)
      }
      .padding(.vertical, inset)
      .padding(.leading, inset)
      .padding(.trailing, trailing)
      .background(isHovered ? Palette.accent : .clear, in: .capsule)
      .contentShape(.capsule)
    }
    .buttonStyle(.plain)
    .onHover { isHovered = $0 }
    .animation(.easeOut(duration: 0.12), value: isHovered)
    .accessibilityLabel("\(agent.name), Agent Settings")
  }
}
