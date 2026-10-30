import SwiftUI

struct AgentSetupView: View {
  let model: AppModel
  @State private var name = "Goat"
  @State private var instructions = ""

  private var trimmedName: String { name.trimmingCharacters(in: .whitespaces) }

  var body: some View {
    Form {
      Section("Name") {
        TextField("Agent Name", text: $name)
          .submitLabel(.done)
      }
      Section {
        TextField(
          "Instructions",
          text: $instructions,
          prompt: Text("e.g. Keep answers short. Plan my days around the mornings."),
          axis: .vertical
        )
        .lineLimit(3...8)
      } header: {
        Text("Instructions")
      } footer: {
        FormFooter(hint: "You can leave this blank.", error: model.errorMessage)
      }
    }
    .navigationTitle("Create Agent")
    .scrollDismissesKeyboard(.interactively)
    .safeAreaBar(edge: .bottom) {
      PrimaryButton(
        title: "Get Started",
        workingTitle: "Creating…",
        isWorking: model.isWorking,
        isEnabled: !trimmedName.isEmpty,
        action: submit
      )
    }
  }

  private func submit() {
    guard !trimmedName.isEmpty, !model.isWorking else { return }
    let wishes = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
    Task { await model.createAgent(name: trimmedName, instructions: wishes) }
  }
}
