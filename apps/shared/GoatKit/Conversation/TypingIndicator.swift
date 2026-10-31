import SwiftUI

struct TypingIndicator: View {
  var body: some View {
    Image(systemName: "ellipsis")
      .font(.title3.weight(.semibold))
      .foregroundStyle(.secondary)
      .symbolEffect(.variableColor.iterative.dimInactiveLayers)
      .padding(.horizontal, 16)
      .frame(minHeight: 44)
      .background(.fill.secondary, in: .rect(cornerRadius: 22))
      .frame(maxWidth: .infinity, alignment: .leading)
      .accessibilityLabel("Writing a reply")
  }
}
