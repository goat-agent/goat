import Foundation
import GoatAPI
import Observation

@Observable
final class IntegrationsModel {
  private(set) var integrations: [Components.Schemas.Integration] = []
  private(set) var failedIntegrationID: String?
  private(set) var connectingID: String?

  let client: Client

  init(client: Client) {
    self.client = client
  }

  func load() async {
    guard case .ok(let response) = try? await client.listIntegrations(),
      let list = try? response.body.json.integrations
    else { return }
    integrations = list
  }

  func iconURL(of integrationID: String) -> String? {
    integrations.first { $0.id == integrationID }?.iconUrl
  }

  func isConnected(_ integrationID: String) -> Bool {
    integrations.first { $0.id == integrationID }?.connections
      .contains { !$0.needsReconnect } ?? false
  }

  // The system refuses a second web authentication session while one is open.
  func connect(_ integrationID: String, agentID: String? = nil, with connector: Connector) async {
    guard connectingID == nil else { return }
    connectingID = integrationID
    failedIntegrationID = nil
    defer { connectingID = nil }
    let outcome = await connector.connect(integrationID, agentID: agentID)
    if outcome == .failed { failedIntegrationID = integrationID }
    await load()
  }

  func disconnect(_ connection: Components.Schemas.Connection, of integrationID: String) async {
    failedIntegrationID = nil
    let output = try? await client.deleteConnection(path: .init(id: connection.id))
    if case .noContent = output {
      await load()
    } else {
      failedIntegrationID = integrationID
    }
  }
}
