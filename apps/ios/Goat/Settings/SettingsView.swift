import GoatAPI
import SwiftUI

struct SettingsView: View {
  let model: AppModel
  @Environment(\.dismiss) private var dismiss

  var body: some View {
    NavigationStack {
      List {
        Section {
          NavigationLink("연동") { IntegrationsView(client: model.client) }
        }
      }
      .navigationTitle("설정")
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .topBarTrailing) {
          Button(role: .close) { dismiss() }
        }
      }
    }
  }
}
