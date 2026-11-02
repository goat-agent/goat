import SwiftUI

/// The last step of signing up: the first agent, made with the same form as every other one.
struct FirstAgentView: View {
  let model: AppModel
  @State private var name = "Goat"
  @State private var instructions = ""
  @State private var photo: Data?

  private var trimmedName: String { name.trimmingCharacters(in: .whitespaces) }

  var body: some View {
    NavigationStack {
      ScrollView {
        VStack(alignment: .leading, spacing: 12) {
          NewAgentForm(name: $name, instructions: $instructions, photo: $photo)
          if let error = model.errorMessage {
            FormFooter(hint: "", error: error)
          }
        }
        .padding()
      }
      .background(Palette.background)
      .portalHost()
      .navigationTitle("Your Agent")
      .scrollDismissesKeyboard(.interactively)
      .safeAreaBar(edge: .bottom) {
        PrimaryButton(
          title: "Start",
          workingTitle: "Starting…",
          isWorking: model.isWorking,
          isEnabled: !trimmedName.isEmpty,
          action: submit
        )
      }
    }
  }

  private func submit() {
    guard !trimmedName.isEmpty, !model.isWorking else { return }
    let wishes = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
    Task { await model.createAgent(name: trimmedName, instructions: wishes, photo: photo) }
  }
}
