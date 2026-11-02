import SwiftUI

/// What the agent did on the user's computers: one quiet line that opens to the steps.
struct StepList: View {
  let steps: [String]

  @State private var isExpanded = false

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      Button {
        withAnimation(.snappy(duration: 0.2)) { isExpanded.toggle() }
      } label: {
        HStack(spacing: 4) {
          IconSlot(systemName: "desktopcomputer")
          Text("Did \(steps.count) things on your computer")
          Image(systemName: "chevron.right")
            .imageScale(.small)
            .rotationEffect(.degrees(isExpanded ? 90 : 0))
        }
        .contentShape(.rect)
      }
      .buttonStyle(.plain)
      if isExpanded {
        ForEach(Array(steps.enumerated()), id: \.offset) { _, step in
          Text(step)
            .padding(.leading, 24)
            .transition(.opacity)
        }
      }
    }
    .font(.footnote)
    .foregroundStyle(Palette.mutedForeground)
    .frame(maxWidth: .infinity, alignment: .leading)
    .padding(.horizontal, 4)
  }
}
