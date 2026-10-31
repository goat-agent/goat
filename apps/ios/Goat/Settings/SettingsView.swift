import GoatAPI
import SwiftUI

/// Settings: the connected accounts and the user's own account.
struct SettingsView: View {
  let model: AppModel
  @State private var integrations: IntegrationsModel
  @Environment(\.dismiss) private var dismiss

  init(model: AppModel) {
    self.model = model
    _integrations = State(initialValue: IntegrationsModel(client: model.client))
  }

  var body: some View {
    NavigationStack {
      Form {
        IntegrationsSection(integrations: integrations)
        AccountSection(model: model)
      }
      .navigationTitle("Settings")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button(role: .close) { dismiss() }
        }
      }
    }
  }
}
