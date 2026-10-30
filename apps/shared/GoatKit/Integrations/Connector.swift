import AuthenticationServices
import Foundation
import GoatAPI
import SwiftUI

struct Connector {
  enum Outcome {
    case connected
    case cancelled
    case failed
  }

  let client: Client
  let session: WebAuthenticationSession

  func connect(_ integrationID: String, agentID: String? = nil) async -> Outcome {
    do {
      let started = try await client.connectIntegration(
        path: .init(id: integrationID),
        body: .json(.init(agentId: agentID))
      )
      guard case .ok(let response) = started, let url = URL(string: try response.body.json.url)
      else { return .failed }

      let callback = try await session.authenticate(
        using: url,
        callbackURLScheme: "goat",
        preferredBrowserSession: .shared
      )
      let query = URLComponents(url: callback, resolvingAgainstBaseURL: false)?.queryItems ?? []
      let value = { (name: String) in query.first { $0.name == name }?.value }
      if let error = value("error") { return error == "denied" ? .cancelled : .failed }
      guard let state = value("state"), let code = value("code") else { return .failed }

      let finished = try await client.createConnection(
        path: .init(id: integrationID),
        body: .json(.init(state: state, code: code))
      )
      if case .created = finished { return .connected }
      return .failed
    } catch let error as ASWebAuthenticationSessionError where error.code == .canceledLogin {
      return .cancelled
    } catch {
      return .failed
    }
  }
}
