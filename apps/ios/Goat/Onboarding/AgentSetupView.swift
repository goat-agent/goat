import SwiftUI

struct AgentSetupView: View {
  let model: AppModel
  @State private var name = "Goat"
  @State private var instructions = ""

  private var trimmedName: String { name.trimmingCharacters(in: .whitespaces) }

  var body: some View {
    Form {
      Section("이름") {
        TextField("에이전트 이름", text: $name)
          .submitLabel(.done)
      }
      Section {
        TextField(
          "바라는 점",
          text: $instructions,
          prompt: Text("예: 답은 짧게 해 줘. 일정은 오전 위주로 잡아 줘."),
          axis: .vertical
        )
        .lineLimit(3...8)
      } header: {
        Text("바라는 점")
      } footer: {
        FormFooter(hint: "비워 두셔도 돼요.", error: model.errorMessage)
      }
    }
    .navigationTitle("에이전트 만들기")
    .scrollDismissesKeyboard(.interactively)
    .safeAreaBar(edge: .bottom) {
      PrimaryButton(
        title: "시작하기",
        workingTitle: "만드는 중…",
        isWorking: model.isWorking,
        isEnabled: !trimmedName.isEmpty,
        action: submit
      )
    }
  }

  private func submit() {
    guard !trimmedName.isEmpty, !model.isWorking else { return }
    let wishes = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
    Task { await model.createAgent(name: trimmedName, instructions: wishes) }
  }
}
