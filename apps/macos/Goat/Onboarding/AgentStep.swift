import SwiftUI

/// Naming a new agent: the last step of getting started, and the New Agent sheet.
struct AgentStep: View {
  let model: AppModel
  var cancel: (() -> Void)?
  @State private var name = "Goat"
  @State private var instructions = ""
  @FocusState private var isFocused: Bool

  private var trimmedName: String { name.trimmingCharacters(in: .whitespaces) }

  var body: some View {
    OnboardingCard(
      title: "Meet your agent",
      message: Text("Give it a name. Instructions are optional."),
      action: "Create Agent",
      workingAction: "Creating…",
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
        prompt: Text("e.g. Keep answers short. Plan my days around the mornings."),
        axis: .vertical
      )
      .labelsHidden()
      .lineLimit(3...6)
    } links: {
      if let cancel {
        Button("Cancel", action: cancel)
          .keyboardShortcut(.cancelAction)
      }
    }
    .onAppear { isFocused = true }
  }

  private func submit() {
    guard !trimmedName.isEmpty, !model.isWorking else { return }
    let wishes = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
    Task { await model.createAgent(name: trimmedName, instructions: wishes) }
  }
}
