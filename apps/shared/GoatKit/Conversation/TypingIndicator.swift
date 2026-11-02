import SwiftUI

struct TypingIndicator: View {
  var body: some View {
    Bubble {
      Image(systemName: "ellipsis")
        .fontWeight(.bold)
        .foregroundStyle(Palette.mutedForeground)
        .symbolEffect(.variableColor.iterative.dimInactiveLayers)
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .accessibilityLabel("Writing a reply")
  }
}
