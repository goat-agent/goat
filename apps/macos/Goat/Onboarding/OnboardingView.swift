import SwiftUI

/// Signing in and the first setup, one step at a time in a small window.
struct OnboardingView: View {
  @Bindable var model: AppModel

  private enum Step: Hashable {
    case email
    case code(String)
    case agent
  }

  private var step: Step {
    switch model.screen {
    case .setup: .agent
    default: model.signInPath.last.map(Step.code) ?? .email
    }
  }

  var body: some View {
    Group {
      switch step {
      case .email: EmailStep(model: model)
      case .code(let email): CodeStep(model: model, email: email)
      case .agent: AgentStep(model: model)
      }
    }
    .id(step)
    .transition(.opacity)
    .animation(.default, value: step)
    .toolbar(removing: .title)
    .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
  }
}

private struct EmailStep: View {
  let model: AppModel
  @State private var email = ""
  @FocusState private var isFocused: Bool

  private var address: String { email.trimmingCharacters(in: .whitespaces) }
  private var canSubmit: Bool { address.contains("@") && address.contains(".") }

  var body: some View {
    OnboardingCard(
      title: "Welcome",
      action: "Continue",
      workingAction: "Sending…",
      isWorking: model.isWorking,
      canSubmit: canSubmit,
      error: model.errorMessage,
      submit: submit
    ) {
      TextField("Email", text: $email, prompt: Text(verbatim: "you@example.com"))
        .labelsHidden()
        .textContentType(.emailAddress)
        .autocorrectionDisabled()
        .focused($isFocused)
        .onSubmit(submit)
    }
    .onAppear { isFocused = true }
  }

  private func submit() {
    guard canSubmit, !model.isWorking else { return }
    Task { await model.sendCode(to: address) }
  }
}

private struct CodeStep: View {
  @Bindable var model: AppModel
  let email: String
  @State private var code = ""
  @State private var resendAt = Date.now.addingTimeInterval(30)
  @FocusState private var isFocused: Bool

  var body: some View {
    OnboardingCard(
      title: "Check your email",
      message: Text("Enter the 6-digit code we sent to \(email)."),
      action: "Sign In",
      workingAction: "Checking…",
      isWorking: model.isWorking,
      canSubmit: code.count == 6,
      error: model.errorMessage,
      submit: submit
    ) {
      TextField("6-Digit Code", text: $code, prompt: Text(verbatim: "000000"))
        .labelsHidden()
        .textContentType(.oneTimeCode)
        .font(.title2.monospacedDigit())
        .multilineTextAlignment(.center)
        .focused($isFocused)
        .onChange(of: code) { _, newValue in
          let digits = String(newValue.filter(\.isNumber).prefix(6))
          if digits != newValue { code = digits }
          if digits.count == 6 { submit() }
        }
    } links: {
      TimelineView(.periodic(from: .now, by: 1)) { context in
        let remaining = max(0, Int(resendAt.timeIntervalSince(context.date).rounded(.up)))
        Button(remaining > 0 ? "Resend Code in \(remaining)s" : "Resend Code") {
          resendAt = .now.addingTimeInterval(30)
          Task { await model.sendCode(to: email) }
        }
        .disabled(remaining > 0 || model.isWorking)
      }
      Button("Use a Different Email") { model.signInPath = [] }
    }
    .onChange(of: model.failedAttempts) { code = "" }
    .onAppear { isFocused = true }
  }

  private func submit() {
    guard code.count == 6, !model.isWorking else { return }
    Task { await model.signIn(email: email, code: code) }
  }
}
