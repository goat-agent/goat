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
    VStack(alignment: .leading, spacing: 18) {
      Text("New Agent")
        .font(.title3.weight(.semibold))
        .foregroundStyle(Palette.foreground)
      NewAgentForm(name: $name, instructions: $instructions, photo: $photo)
      if let error = model.errorMessage {
        FormFooter(hint: "", error: error)
      }
      HStack(spacing: 8) {
        Button {
          dismiss()
        } label: {
          Text("Cancel").frame(maxWidth: .infinity)
        }
        .buttonStyle(.secondary)
        .keyboardShortcut(.cancelAction)
        Button(action: create) {
          Text("Create").frame(maxWidth: .infinity)
        }
        .buttonStyle(.primary)
        .keyboardShortcut(.defaultAction)
        .disabled(trimmedName.isEmpty || model.isWorking)
      }
    }
    .padding(22)
    #if os(macOS)
      .frame(width: 380)
    #else
      .frame(maxHeight: .infinity, alignment: .top)
    #endif
    .presentationBackground(Palette.popover)
    .portalHost()
    .interactiveDismissDisabled(hasChanges)
  }

  private func create() {
    let wishes = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
    Task { await model.createAgent(name: trimmedName, instructions: wishes, photo: photo) }
  }
}
