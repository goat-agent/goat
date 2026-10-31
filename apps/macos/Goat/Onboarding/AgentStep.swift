import SwiftUI

/// Naming the first agent: the last step of getting started.
struct AgentStep: View {
  let model: AppModel
  @State private var name = "Goat"
  @State private var instructions = ""
  @FocusState private var isFocused: Bool

  private var trimmedName: String { name.trimmingCharacters(in: .whitespaces) }

  var body: some View {
    OnboardingCard(
      title: "Your Agent",
      action: "Start",
      workingAction: "Starting…",
      isWorking: model.isWorking,
      canSubmit: !trimmedName.isEmpty,
      error: model.errorMessage,
      submit: submit
    ) {
      TextField("Agent Name", text: $name)
        .labelsHidden()
        .focused($isFocused)
      TextField(
        "Instructions",
        text: $instructions,
        axis: .vertical
      )
      .labelsHidden()
      .lineLimit(3...6)
    }
    .onAppear { isFocused = true }
  }

  private func submit() {
    guard !trimmedName.isEmpty, !model.isWorking else { return }
    let wishes = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
    Task { await model.createAgent(name: trimmedName, instructions: wishes, photo: nil) }
  }
}
