import SwiftUI

struct EmailView: View {
  let model: AppModel
  @State private var email = ""
  @FocusState private var isFocused: Bool

  private var address: String { email.trimmingCharacters(in: .whitespaces) }
  private var canSubmit: Bool { address.contains("@") && address.contains(".") }

  var body: some View {
    Form {
      Section {
        TextField("Email", text: $email, prompt: Text(verbatim: "you@example.com"))
          .textContentType(.emailAddress)
          .keyboardType(.emailAddress)
          .textInputAutocapitalization(.never)
          .autocorrectionDisabled()
          .submitLabel(.continue)
          .focused($isFocused)
          .onSubmit(submit)
      } footer: {
        FormFooter(hint: "We’ll send a sign-in code to this address.", error: model.errorMessage)
      }
    }
    .navigationTitle("Get Started with Goat")
    .scrollDismissesKeyboard(.interactively)
    .safeAreaBar(edge: .bottom) {
      PrimaryButton(
        title: "Send Code",
        workingTitle: "Sending…",
        isWorking: model.isWorking,
        isEnabled: canSubmit,
        action: submit
      )
    }
    .onAppear { isFocused = true }
  }

  private func submit() {
    guard canSubmit, !model.isWorking else { return }
    Task { await model.sendCode(to: address) }
  }
}
