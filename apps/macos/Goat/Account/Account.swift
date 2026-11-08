import Foundation
import GoatAPI

@Observable
final class Account {
  enum Failure: Error {
    case invalidEmail, wrongCode, unreachable
  }

  private static let tokenKey = "session-token"

  private(set) var token = Keychain.read(Account.tokenKey)

  var isSignedIn: Bool { token != nil }

  func sendCode(to email: String) async throws(Failure) {
    let output: Operations.SendCode.Output
    do {
      output = try await client.sendCode(body: .json(.init(email: email)))
    } catch {
      throw .unreachable
    }
    switch output {
    case .noContent: return
    case .badRequest: throw .invalidEmail
    default: throw .unreachable
    }
  }

  func signIn(email: String, code: String) async throws(Failure) {
    let output: Operations.SignIn.Output
    do {
      output = try await client.signIn(
        body: .json(.init(email: email, code: code, device: Installation.device)))
    } catch {
      throw .unreachable
    }
    switch output {
    case .ok(let ok):
      guard case .json(let session) = ok.body else { throw .unreachable }
      Keychain.write(session.token, for: Self.tokenKey)
      token = session.token
    case .badRequest, .unauthorized: throw .wrongCode
    default: throw .unreachable
    }
  }

  func signOut() async {
    _ = try? await client.signOut()
    forget()
  }

  /// Drops a session the server no longer accepts.
  func forget() {
    Keychain.delete(Self.tokenKey)
    token = nil
  }

  private var client: Client {
    let token = token
    return .goat(serverURL: .goatServer, token: { token })
  }
}
