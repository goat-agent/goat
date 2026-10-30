import SwiftUI

/// What the agent did on the user's computers, one quiet line per step.
struct StepList: View {
  let steps: [String]

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      ForEach(Array(steps.enumerated()), id: \.offset) { _, step in
        Label(step, systemImage: "desktopcomputer")
          .font(.footnote)
          .foregroundStyle(.secondary)
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
    .padding(.horizontal, 4)
    .accessibilityElement(children: .combine)
  }
}
