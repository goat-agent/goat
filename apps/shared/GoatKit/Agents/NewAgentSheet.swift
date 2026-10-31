import SwiftUI

/// Making an agent, from the + on either device. The picture waits on this device until the
/// agent exists.
struct NewAgentSheet: View {
  let model: AppModel
  @State private var name = ""
  @State private var instructions = ""
  @State private var photo: Data?
  @Environment(\.dismiss) private var dismiss

  private var trimmedName: String { name.trimmingCharacters(in: .whitespaces) }
  private var hasChanges: Bool { !trimmedName.isEmpty || !instructions.isEmpty || photo != nil }

  var body: some View {
    NavigationStack {
      Form {
        NewAgentForm(name: $name, instructions: $instructions, photo: $photo)
        if let error = model.errorMessage {
          Section { FormFooter(hint: "", error: error) }
        }
      }
      .formStyle(.grouped)
      .navigationTitle("New Agent")
      .toolbarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button(role: .close) { dismiss() }
        }
        ToolbarItem(placement: .confirmationAction) {
          Button(role: .confirm, action: create)
            .disabled(trimmedName.isEmpty || model.isWorking)
        }
      }
      .interactiveDismissDisabled(hasChanges)
    }
  }

  private func create() {
    let wishes = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
    Task { await model.createAgent(name: trimmedName, instructions: wishes, photo: photo) }
  }
}
