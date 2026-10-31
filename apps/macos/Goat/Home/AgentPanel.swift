import GoatAPI
import SwiftUI

/// The open agent's settings, beside its conversation; the form is the iPhone's agent sheet.
struct AgentPanel: View {
  let model: AppModel
  let agent: Components.Schemas.Agent
  let close: () -> Void

  var body: some View {
    AgentSettingsForm(model: model, agent: agent, onDelete: close)
      .scrollContentBackground(.hidden)
      .safeAreaPadding(.top, 36)
      .overlay(alignment: .topTrailing) {
        Button("Close", systemImage: "xmark", action: close)
          .labelStyle(.iconOnly)
          .buttonStyle(.glass)
          .buttonBorderShape(.circle)
          .padding(10)
      }
  }
}
