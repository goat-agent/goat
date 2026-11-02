import SwiftUI

/// web's `Bubble`: one message. The user's own are `primary`; the agent's are `secondary`. The
/// corner is half a one-line bubble's height, kept as it grows.
struct Bubble<Content: View>: View {
  enum Variant {
    case primary
    case secondary
  }

  var variant = Variant.secondary
  @ViewBuilder let content: Content

  #if os(macOS)
    private let padding = EdgeInsets(top: 8, leading: 12, bottom: 8, trailing: 12)
    private let radius: CGFloat = 17
  #else
    private let padding = EdgeInsets(top: 10, leading: 14, bottom: 10, trailing: 14)
    private let radius: CGFloat = 21
  #endif

  var body: some View {
    content
      .padding(padding)
      .foregroundStyle(variant == .primary ? Palette.primaryForeground : Palette.foreground)
      .background(
        variant == .primary ? Palette.primary : Palette.secondary,
        in: .rect(cornerRadius: radius, style: .continuous))
  }
}

#Preview {
  VStack(alignment: .leading, spacing: 4) {
    Bubble { Text(verbatim: "I gathered last week’s notes and the related email.") }
    Bubble(variant: .primary) { Text(verbatim: "Thanks. Post it to Slack too") }
  }
  .padding(24)
  .background(Palette.background)
}
