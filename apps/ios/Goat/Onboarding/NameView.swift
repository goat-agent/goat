import SwiftUI

struct NameView: View {
  let model: AppModel
  @State private var name = ""
  @FocusState private var isFocused: Bool

  private var trimmed: String { name.trimmingCharacters(in: .whitespaces) }

  var body: some View {
    Form {
      Section {
        TextField("Name", text: $name, prompt: Text("e.g. Alex"))
          .textContentType(.givenName)
          .submitLabel(.continue)
          .focused($isFocused)
          .onSubmit(submit)
      } footer: {
        FormFooter(hint: "Your agent will call you by this name.", error: model.errorMessage)
      }
    }
    .navigationTitle("Name")
    .safeAreaBar(edge: .bottom) {
      PrimaryButton(
        title: "Next",
        workingTitle: "Saving…",
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
