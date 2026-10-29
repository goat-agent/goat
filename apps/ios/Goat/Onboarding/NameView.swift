import SwiftUI

struct NameView: View {
  let model: AppModel
  @State private var name = ""
  @FocusState private var isFocused: Bool

  private var trimmed: String { name.trimmingCharacters(in: .whitespaces) }

  var body: some View {
    Form {
      Section {
        TextField("이름", text: $name, prompt: Text("예: 민호"))
          .textContentType(.givenName)
          .submitLabel(.continue)
          .focused($isFocused)
          .onSubmit(submit)
      } footer: {
        FormFooter(hint: "에이전트가 이 이름으로 불러요.", error: model.errorMessage)
      }
    }
    .navigationTitle("이름")
    .safeAreaBar(edge: .bottom) {
      PrimaryButton(
        title: "다음",
        workingTitle: "저장 중…",
        isWorking: model.isWorking,
        isEnabled: !trimmed.isEmpty,
        action: submit
      )
    }
    .onAppear { isFocused = true }
  }

  private func submit() {
    guard !trimmed.isEmpty, !model.isWorking else { return }
    Task { await model.setName(trimmed) }
  }
}
