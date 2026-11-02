import SwiftUI

struct CodeView: View {
  let model: AppModel
  let email: String
  @State private var code = ""
  @State private var resendAt = Date.now.addingTimeInterval(30)

  var body: some View {
    VStack(alignment: .leading, spacing: 16) {
      Text("Sent to \(email).")
        .foregroundStyle(Palette.mutedForeground)
      InputOTP(code: $code)
        .disabled(model.isWorking)
        .onChange(of: code) { _, newValue in
          if newValue.count == 6 { submitSoon() }
        }
      if let error = model.errorMessage {
        FormFooter(hint: "", error: error)
      }
      TimelineView(.periodic(from: .now, by: 1)) { context in
        let remaining = max(0, Int(resendAt.timeIntervalSince(context.date).rounded(.up)))
        Button(remaining > 0 ? "Resend Code in \(remaining)s" : "Resend Code") {
          resendAt = .now.addingTimeInterval(30)
          Task { await model.sendCode(to: email) }
        }
        .buttonStyle(.inline)
        .controlSize(.small)
        .disabled(remaining > 0 || model.isWorking)
        .contentTransition(.numericText(countsDown: true))
      }
    }
    .padding(.horizontal)
    .padding(.top, 8)
    .frame(maxHeight: .infinity, alignment: .top)
    .background(Palette.background)
    .navigationTitle("Enter Code")
    .sensoryFeedback(.error, trigger: model.failedAttempts)
    .onChange(of: model.failedAttempts) { code = "" }
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
