import Foundation

/// Ties the account to the device connection: signed in, this Mac stays reachable.
@Observable
final class AppModel {
  let account = Account()
  let permissions = Permissions()
  let computerUse: ComputerUse
  let connection = DeviceConnection()

  init() {
    computerUse = ComputerUse(permissions: permissions)
    connectIfSignedIn()
  }

  func didSignIn() {
    connectIfSignedIn()
  }

  func signOut() async {
    connection.stop()
    await account.signOut()
  }

  private func connectIfSignedIn() {
    guard let token = account.token else { return }
    connection.start(token: token, answer: computerUse.answer) { [weak self] in
      guard let self else { return }
      connection.stop()
      account.forget()
    }
  }
}
