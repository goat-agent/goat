import SwiftUI

struct TypingIndicator: View {
  var body: some View {
    Image(systemName: "ellipsis")
      .font(.title3.weight(.semibold))
      .foregroundStyle(.secondary)
      .symbolEffect(.variableColor.iterative.dimInactiveLayers)
      .padding(.horizontal, 16)
      .padding(.vertical, 14)
      .background(.fill.secondary, in: .rect(cornerRadius: 20))
      .frame(maxWidth: .infinity, alignment: .leading)
      .accessibilityLabel("Writing a reply")
  }
}
