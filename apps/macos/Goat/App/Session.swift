import Observation

/// Who is signed in on this Mac. The web app signs in and out through `AppScheme`, which hands the
/// session token here on its way past, so the token lives in the Keychain and never on the page.
@Observable
final class Session {
  @ObservationIgnored private let tokens = TokenStore()
  private(set) var token: String?

  var isSignedIn: Bool { token != nil }

  init() {
    token = tokens.load()
  }

  func signIn(token: String) {
    tokens.save(token)
    self.token = token
  }

  func signOut() {
    tokens.clear()
    token = nil
  }
}
