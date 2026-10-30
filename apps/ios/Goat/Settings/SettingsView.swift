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
          NavigationLink("연동") { IntegrationsView(client: model.client) }
        }
        Section {
          Button("로그아웃", role: .destructive) { isConfirmingSignOut = true }
            .confirmationDialog("로그아웃할까요?", isPresented: $isConfirmingSignOut) {
              Button("로그아웃", role: .destructive) {
                Task { await model.signOut() }
              }
            } message: {
              Text("다시 들어오려면 이메일로 받은 코드가 필요해요.")
            }
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
