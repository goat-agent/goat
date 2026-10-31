import GoatAPI
import SwiftUI

/// One agent's settings. Changes save as they are made, so the sheet needs only a close button.
struct AgentSheet: View {
  let model: AppModel
  let agent: Components.Schemas.Agent
  @Environment(\.dismiss) private var dismiss

  var body: some View {
    NavigationStack {
      AgentSettingsForm(model: model, agent: agent) { dismiss() }
        .toolbarTitleDisplayMode(.inline)
        .toolbar {
          ToolbarItem(placement: .cancellationAction) {
            Button(role: .close) { dismiss() }
          }
        }
    }
  }
}
