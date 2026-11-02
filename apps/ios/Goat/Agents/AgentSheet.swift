import GoatAPI
import SwiftUI

/// One agent's settings, a section per kind of setting as in Settings. Changes save as they are
/// made, so the sheet needs only a close button.
struct AgentSheet: View {
  let model: AppModel
  let agent: Components.Schemas.Agent
  @State private var schedules: SchedulesModel
  @Environment(\.dismiss) private var dismiss

  init(model: AppModel, agent: Components.Schemas.Agent) {
    self.model = model
    self.agent = agent
    _schedules = State(initialValue: SchedulesModel(agentID: agent.id, client: model.client))
  }

  var body: some View {
    NavigationStack {
      ScrollView {
        VStack(alignment: .leading, spacing: 14) {
          Text("General")
            .font(.sectionTitle)
            .foregroundStyle(Palette.foreground)
          AgentSettingsForm(model: model, agent: agent) { dismiss() }
          Text("Schedules")
            .font(.sectionTitle)
            .foregroundStyle(Palette.foreground)
            .padding(.top, 18)
          SchedulesSection(model: schedules)
        }
        .padding()
      }
      .background(Palette.background)
      .portalHost()
      .toolbarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button(role: .close) { dismiss() }
        }
      }
    }
  }
}
