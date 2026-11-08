import SwiftUI

struct SignInView: View {
  let account: Account
  let onSignIn: () -> Void

  @State private var email = ""
  @State private var code = ""
  @State private var isCodeSent = false
  @State private var isWorking = false
  @State private var error: LocalizedStringKey?

  var body: some View {
    VStack(alignment: .leading, spacing: 10) {
      if isCodeSent {
        Text("Enter the code")
          .font(.headline)
        Text("We sent a 6-digit code to \(email).")
          .font(.callout)
          .foregroundStyle(.secondary)
        TextField("6-digit code", text: $code)
          .textContentType(.oneTimeCode)
          .onSubmit(signIn)
      } else {
        Text("Sign in to Goat")
          .font(.headline)
        Text("Use the email of your Goat account.")
          .font(.callout)
          .foregroundStyle(.secondary)
        TextField("Email", text: $email)
          .textContentType(.emailAddress)
          .onSubmit(sendCode)
      }
      if let error {
        Text(error)
          .font(.callout)
          .foregroundStyle(.red)
      }
      HStack {
        if isCodeSent {
          Button("Use a Different Email", action: startOver)
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
          Spacer()
          Button("Sign In", action: signIn)
            .keyboardShortcut(.defaultAction)
            .disabled(isWorking || code.count != 6)
        } else {
          Spacer()
          Button("Continue", action: sendCode)
            .keyboardShortcut(.defaultAction)
            .disabled(isWorking || email.isEmpty)
        }
      }
    }
    .textFieldStyle(.roundedBorder)
  }

  private func startOver() {
    isCodeSent = false
    code = ""
    error = nil
  }

  private func sendCode() {
    run {
      try await account.sendCode(to: email.trimmingCharacters(in: .whitespaces))
      isCodeSent = true
    }
  }

  private func signIn() {
    run {
      try await account.signIn(email: email.trimmingCharacters(in: .whitespaces), code: code)
      onSignIn()
    }
  }

  private func run(_ body: @escaping () async throws -> Void) {
    isWorking = true
    error = nil
    Task {
      do {
        try await body()
      } catch Account.Failure.invalidEmail {
        error = "Check the email address."
      } catch Account.Failure.wrongCode {
        error = "That code is not right. Check it and try again."
        code = ""
      } catch {
        self.error = "Couldn’t reach Goat. Try again in a moment."
      }
      isWorking = false
    }
  }
}
