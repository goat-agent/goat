import SwiftUI

/// Goat's Settings window: this Mac's computer use, and the services the agent can reach.
struct SettingsView: View {
  let model: AppModel
  @Bindable var computer: ComputerService

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
        NavigationStack { IntegrationsView(client: model.client) }
      }
    }
    .frame(width: 520, height: 460)
  }
}
