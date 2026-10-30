import SwiftUI

/// An agent's face: Goat on a background color that follows from the agent's id, so each agent
/// keeps its color on every device.
struct AgentAvatar: View {
  let agentID: String
  let size: CGFloat

  private static let colors: [Color] = [.yellow, .green, .orange, .pink, .mint, .teal, .cyan, .red]

  var body: some View {
    Image("Goat")
      .resizable()
      .scaledToFit()
      .padding(size * 0.08)
      .frame(width: size, height: size)
      .background(Self.color(for: agentID).opacity(0.8), in: .circle)
      .accessibilityHidden(true)
  }

  private static func color(for id: String) -> Color {
    // A stable hash: Swift's own hashing changes from launch to launch.
    let sum = id.unicodeScalars.reduce(0) { ($0 &* 31 &+ Int($1.value)) & 0xFFFF }
    return colors[sum % colors.count]
  }
}
