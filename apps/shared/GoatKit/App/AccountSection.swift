import GoatAPI
import SwiftUI

/// The user's own account in Settings: the name agents call them, the email they sign in with,
/// and signing out.
struct AccountSection: View {
  let model: AppModel
  @State private var name: String
  @State private var isConfirmingSignOut = false

  init(model: AppModel) {
    self.model = model
    _name = State(initialValue: model.userName)
  }

  var body: some View {
    Section("Account") {
      LabeledContent("Name") {
        TextField("Name", text: $name)
          .labelsHidden()
          .multilineTextAlignment(.trailing)
          .submitLabel(.done)
          .onSubmit(saveName)
      }
      LabeledContent("Email", value: model.me?.email ?? "")
    }
    .onDisappear(perform: saveName)

    Section {
      // A Mac form draws a destructive button in plain text unless it is told the color.
      Button("Sign Out", role: .destructive) { isConfirmingSignOut = true }
        .foregroundStyle(.red)
        .confirmationDialog("Sign out?", isPresented: $isConfirmingSignOut) {
          Button("Sign Out", role: .destructive) {
            Task { await model.signOut() }
          }
        } message: {
          Text("To sign back in, you’ll need a code from your email.")
        }
    }
  }

  private func saveName() {
    let trimmed = name.trimmingCharacters(in: .whitespaces)
    guard trimmed != model.userName else { return }
    Task { await model.rename(to: trimmed) }
  }
}
