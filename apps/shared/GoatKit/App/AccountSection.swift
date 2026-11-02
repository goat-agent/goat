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

  private var email: String { model.me?.email ?? "" }

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      Text("Account")
        .font(.sectionTitle)
        .foregroundStyle(Palette.foreground)
      HStack(alignment: .bottom, spacing: 14) {
        AvatarCircle(name: name.isEmpty ? email : name, image: nil, size: 56)
        VStack(alignment: .leading, spacing: 6) {
          FieldLabel("Name")
          TextField("Name", text: $name, prompt: Text(verbatim: ""))
            .labelsHidden()
            .submitLabel(.done)
            .onSubmit(saveName)
            .inputStyle()
        }
      }
      ItemGroup {
        Item(title: String(localized: "Email")) {
          Text(verbatim: email)
            .foregroundStyle(Palette.mutedForeground)
        }
      }
      Button("Sign Out", role: .destructive) { isConfirmingSignOut = true }
        .buttonStyle(.destructive)
        .padding(.top, 4)
    }
    .alertDialog(
      "Sign out?", isPresented: $isConfirmingSignOut,
      message: "To sign back in, you’ll need a code from your email.", action: "Sign Out"
    ) {
      Task { await model.signOut() }
    }
    .onDisappear(perform: saveName)
  }

  private func saveName() {
    let trimmed = name.trimmingCharacters(in: .whitespaces)
    guard trimmed != model.userName else { return }
    Task { await model.rename(to: trimmed) }
  }
}
