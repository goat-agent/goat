import SwiftUI

/// Goat's Settings window: this Mac's computer use, the services the agent can reach, and the
/// user's account.
struct SettingsView: View {
  let model: AppModel
  @Bindable var computer: ComputerService
  @State private var integrations: IntegrationsModel

  init(model: AppModel, computer: ComputerService) {
    self.model = model
    self.computer = computer
    _integrations = State(initialValue: IntegrationsModel(client: model.client))
  }

  var body: some View {
    TabView {
      Tab("This Mac", systemImage: "desktopcomputer") {
        Form {
          Section {
            Toggle("Let the agent use this Mac", isOn: $computer.isEnabled)
              .disabled(!computer.permissions.areGranted)
          } footer: {
            Text(
              "The agent works in the background when it can. When it needs your pointer for a moment, a ring follows it; press Esc to stop."
            )
          }
          Section("Permissions") {
            PermissionRows(permissions: computer.permissions)
          }
        }
        .formStyle(.grouped)
        .task { await computer.permissions.watch() }
      }
      Tab("Integrations", systemImage: "puzzlepiece.extension") {
        NavigationStack {
          Form { IntegrationsSection(integrations: integrations) }
            .formStyle(.grouped)
        }
      }
      Tab("Account", systemImage: "person.crop.circle") {
        Form { AccountSection(model: model) }
          .formStyle(.grouped)
      }
    }
    .frame(width: 520, height: 460)
  }
}
