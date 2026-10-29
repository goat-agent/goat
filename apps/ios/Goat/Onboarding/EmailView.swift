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
        TextField("이메일", text: $email, prompt: Text("you@example.com"))
          .textContentType(.emailAddress)
          .keyboardType(.emailAddress)
          .textInputAutocapitalization(.never)
          .autocorrectionDisabled()
          .submitLabel(.continue)
          .focused($isFocused)
          .onSubmit(submit)
      } footer: {
        FormFooter(hint: "이 주소로 로그인 코드를 보내 드릴게요.", error: model.errorMessage)
      }
    }
    .navigationTitle("Goat 시작하기")
    .scrollDismissesKeyboard(.interactively)
    .safeAreaBar(edge: .bottom) {
      PrimaryButton(
        title: "코드 받기",
        workingTitle: "보내는 중…",
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
