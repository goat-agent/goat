import GoatAPI
import SwiftUI

/// Settings: the connected accounts, where the account is signed in, and the account itself.
struct SettingsView: View {
  let model: AppModel
  @State private var integrations: IntegrationsModel
  @State private var devices: DevicesModel
  @Environment(\.dismiss) private var dismiss

  init(model: AppModel) {
    self.model = model
    _integrations = State(initialValue: IntegrationsModel(client: model.client))
    _devices = State(initialValue: DevicesModel(client: model.client))
  }

  var body: some View {
    NavigationStack {
      ScrollView {
        VStack(alignment: .leading, spacing: 36) {
          IntegrationsSection(integrations: integrations)
          DevicesSection(devices: devices)
          AccountSection(model: model)
        }
        .padding()
      }
      .background(Palette.background)
      .portalHost()
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
