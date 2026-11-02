import GoatAPI
import SwiftUI

/// An agent's settings: the iPhone's agent sheet and the Mac's panel beside the conversation.
/// Changes save shortly after they are made and when the form goes away, so it needs no Save.
struct AgentSettingsForm: View {
  let model: AppModel
  /// The agent as it is now; show the form with `.id(agent.id)` so another agent starts fresh.
  let agent: Components.Schemas.Agent
  let onDelete: () -> Void

  @State private var name: String
  @State private var instructions: String
  @State private var isConfirmingDelete = false
  @State private var schedules: SchedulesModel

  init(model: AppModel, agent: Components.Schemas.Agent, onDelete: @escaping () -> Void) {
    self.model = model
    self.agent = agent
    self.onDelete = onDelete
    _name = State(initialValue: agent.name)
    _instructions = State(initialValue: agent.instructions)
    _schedules = State(initialValue: SchedulesModel(agentID: agent.id, client: model.client))
  }

  var body: some View {
    Form {
      AgentForm(
        name: $name,
        instructions: $instructions,
        hasPhoto: agent.hasPhoto,
        pickPhoto: { photo in Task { await model.setPhoto(photo, of: agent.id) } },
        removePhoto: { Task { await model.removePhoto(of: agent.id) } }
      ) {
        AvatarView(agent: agent, size: 96)
      }

      SchedulesSection(model: schedules)

      Section {
        // A Mac form draws a destructive button in plain text unless it is told the color.
        Button("Delete Agent", role: .destructive) { isConfirmingDelete = true }
          .foregroundStyle(.red)
          .confirmationDialog(
            "Delete \(agent.name)?",
            isPresented: $isConfirmingDelete,
            titleVisibility: .visible
          ) {
            Button("Delete Agent", role: .destructive) {
              onDelete()
              Task { await model.delete(agent.id) }
            }
          } message: {
            Text("The conversation is deleted too.")
          }
      }
    }
    .formStyle(.grouped)
    .task { await schedules.load() }
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
