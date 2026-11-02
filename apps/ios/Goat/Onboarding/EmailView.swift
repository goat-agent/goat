import SwiftUI

struct EmailView: View {
  let model: AppModel
  @State private var email = ""
  @FocusState private var isFocused: Bool

  private var address: String { email.trimmingCharacters(in: .whitespaces) }
  private var canSubmit: Bool { address.contains("@") && address.contains(".") }

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      TextField("Email", text: $email, prompt: Text(verbatim: "you@example.com"))
        .textContentType(.emailAddress)
        .keyboardType(.emailAddress)
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        .submitLabel(.continue)
        .focused($isFocused)
        .onSubmit(submit)
        .inputStyle()
      if let error = model.errorMessage {
        FormFooter(hint: "", error: error)
          .padding(.leading, 16)
      }
    }
    .padding(.horizontal)
    .padding(.top, 8)
    .frame(maxHeight: .infinity, alignment: .top)
    .background(Palette.background)
    .navigationTitle("Welcome")
    .safeAreaBar(edge: .bottom) {
      PrimaryButton(
        title: "Continue",
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
