import SwiftUI

/// The first agent: the last step of getting started, with the same form as every other agent.
struct AgentStep: View {
  let model: AppModel
  @State private var name = "Goat"
  @State private var instructions = ""
  @State private var photo: Data?

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
      NewAgentForm(name: $name, instructions: $instructions, photo: $photo)
        .controlSize(.regular)
    }
  }

  private func submit() {
    guard !trimmedName.isEmpty, !model.isWorking else { return }
    let wishes = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
    Task { await model.createAgent(name: trimmedName, instructions: wishes, photo: photo) }
  }
}
