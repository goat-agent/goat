import GoatAPI
import SwiftUI

/// The open agent at the top of its conversation: its face and name in a glass capsule. Tapping
/// it opens the agent's settings.
struct AgentTitle: View {
  let agent: Components.Schemas.Agent
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      HStack(spacing: 8) {
        AvatarView(agent: agent, size: 32)
        Text(agent.name)
          .font(.headline)
          .lineLimit(1)
      }
      .padding(.vertical, 6)
      .padding(.leading, 6)
      .padding(.trailing, 16)
      .glassEffect(.regular.interactive(), in: .capsule)
    }
    .buttonStyle(.plain)
    .accessibilityLabel("\(agent.name), Agent Settings")
  }
}
