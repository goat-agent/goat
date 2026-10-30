import GoatAPI
import SwiftUI

struct SettingsView: View {
  let model: AppModel
  @State private var isConfirmingSignOut = false
  @Environment(\.dismiss) private var dismiss

  var body: some View {
    NavigationStack {
      List {
        Section {
          NavigationLink("Integrations") { IntegrationsView(client: model.client) }
        }
        Section {
          Button("Sign Out", role: .destructive) { isConfirmingSignOut = true }
            .confirmationDialog("Sign out?", isPresented: $isConfirmingSignOut) {
              Button("Sign Out", role: .destructive) {
                Task { await model.signOut() }
              }
            } message: {
              Text("To sign back in, you’ll need a code from your email.")
            }
        }
      }
      .navigationTitle("Settings")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .topBarTrailing) {
          Button(role: .close) { dismiss() }
        }
      }
    }
  }
}
