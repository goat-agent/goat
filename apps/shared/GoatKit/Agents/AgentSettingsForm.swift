import GoatAPI
import SwiftUI

/// An agent's general settings: the Mac's settings dialog and the iPhone's agent sheet. Changes
/// save shortly after they are made and when the form goes away, so it needs no Save. The
/// container scrolls it.
struct AgentSettingsForm: View {
  let model: AppModel
  /// The agent as it is now; show the form with `.id(agent.id)` so another agent starts fresh.
  let agent: Components.Schemas.Agent
  let onDelete: () -> Void

  @State private var name: String
  @State private var instructions: String
  @State private var isConfirmingDelete = false

  init(model: AppModel, agent: Components.Schemas.Agent, onDelete: @escaping () -> Void) {
    self.model = model
    self.agent = agent
    self.onDelete = onDelete
    _name = State(initialValue: agent.name)
    _instructions = State(initialValue: agent.instructions)
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 24) {
      AgentForm(
        name: $name,
        instructions: $instructions,
        hasPhoto: agent.hasPhoto,
        pickPhoto: { photo in Task { await model.setPhoto(photo, of: agent.id) } },
        removePhoto: { Task { await model.removePhoto(of: agent.id) } }
      ) {
        AvatarView(agent: agent, size: 64)
      }
      Button("Delete Agent") { isConfirmingDelete = true }
        .buttonStyle(.destructive)
    }
    .alertDialog(
      "Delete \(agent.name)?", isPresented: $isConfirmingDelete,
      message: "The conversation is deleted too.", action: "Delete Agent"
    ) {
      onDelete()
      Task { await model.delete(agent.id) }
    }
    .task(id: [name, instructions]) {
      do {
        try await Task.sleep(for: .milliseconds(700))
      } catch {
        return
      }
      await save()
    }
    .onDisappear {
      Task { await save() }
    }
  }

  private func save() async {
    let newName = name.trimmingCharacters(in: .whitespaces)
    let newInstructions = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
    let changedName = !newName.isEmpty && newName != agent.name ? newName : nil
    let changedInstructions = newInstructions != agent.instructions ? newInstructions : nil
    guard changedName != nil || changedInstructions != nil else { return }
    await model.update(agent.id, name: changedName, instructions: changedInstructions)
  }
}
