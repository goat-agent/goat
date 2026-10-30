import SwiftUI

struct CodeView: View {
  let model: AppModel
  let email: String
  @State private var code = ""
  @State private var resendAt = Date.now.addingTimeInterval(30)
  @FocusState private var isFocused: Bool

  var body: some View {
    Form {
      Section {
        TextField("6-Digit Code", text: $code, prompt: Text(verbatim: "000000"))
          .textContentType(.oneTimeCode)
          .keyboardType(.numberPad)
          .font(.title2.monospacedDigit())
          .focused($isFocused)
          .onChange(of: code) { _, newValue in
            let digits = String(newValue.filter(\.isNumber).prefix(6))
            if digits != newValue { code = digits }
            if digits.count == 6 { submitSoon() }
          }
      } footer: {
        FormFooter(hint: "Sent to \(email).", error: model.errorMessage)
      }

      Section {
        TimelineView(.periodic(from: .now, by: 1)) { context in
          let remaining = max(0, Int(resendAt.timeIntervalSince(context.date).rounded(.up)))
          Button(remaining > 0 ? "Resend Code in \(remaining)s" : "Resend Code") {
            resendAt = .now.addingTimeInterval(30)
            Task { await model.sendCode(to: email) }
          }
          .disabled(remaining > 0 || model.isWorking)
          .contentTransition(.numericText(countsDown: true))
        }
      }
    }
    .navigationTitle("Enter Code")
    .safeAreaBar(edge: .bottom) {
      PrimaryButton(
        title: "Sign In",
        workingTitle: "Checking…",
        isWorking: model.isWorking,
        isEnabled: code.count == 6,
        action: submit
      )
    }
    .sensoryFeedback(.error, trigger: model.failedAttempts)
    .onChange(of: model.failedAttempts) { code = "" }
    .onAppear { isFocused = true }
  }

  /// Gives the last digit a moment on screen before the request starts.
  private func submitSoon() {
    Task {
      try? await Task.sleep(for: .milliseconds(250))
      submit()
    }
  }

  private func submit() {
    guard code.count == 6, !model.isWorking else { return }
    Task { await model.signIn(email: email, code: code) }
  }
}
