import AuthenticationServices
import GoatAPI
import SwiftUI

struct IntegrationsView: View {
  @State private var model: IntegrationsModel
  @Environment(\.webAuthenticationSession) private var webAuthenticationSession

  init(client: Client) {
    _model = State(initialValue: IntegrationsModel(client: client))
  }

  var body: some View {
    List {
      Section {
        ForEach(model.integrations, id: \.id) { integration in
          IntegrationRow(
            integration: integration,
            isConnecting: model.connectingID == integration.id,
            canConnect: model.connectingID == nil
          ) {
            connect(integration.id)
          }
          ForEach(integration.connections, id: \.id) { connection in
            AccountRow(connection: connection, canReconnect: model.connectingID == nil) {
              connect(integration.id)
            }
            .swipeActions {
              Button("Disconnect", role: .destructive) {
                Task { await model.disconnect(connection, of: integration.id) }
              }
            }
          }
        }
      } footer: {
        if let failed = model.integrations.first(where: { $0.id == model.failedIntegrationID }) {
          FormFooter(
            hint: "",
            error: String(localized: "Couldn’t connect \(failed.name). Try again in a moment.")
          )
        }
      }
    }
    .navigationTitle("Integrations")
    .task { await model.load() }
  }

  private func connect(_ integrationID: String) {
    let connector = Connector(client: model.client, session: webAuthenticationSession)
    Task { await model.connect(integrationID, with: connector) }
  }
}

private struct IntegrationRow: View {
  let integration: Components.Schemas.Integration
  let isConnecting: Bool
  let canConnect: Bool
  let connect: () -> Void

  var body: some View {
    Button(action: connect) {
      HStack(spacing: 12) {
        IntegrationIcon(url: integration.iconUrl)
        Text(integration.name)
        Spacer(minLength: 8)
        if isConnecting {
          ProgressView()
        } else {
          Image(systemName: "plus")
            .foregroundStyle(.secondary)
        }
      }
    }
    .tint(.primary)
    .disabled(isConnecting || !canConnect)
    .accessibilityLabel(
      integration.connections.isEmpty
        ? "Connect \(integration.name)" : "Add \(integration.name) Account"
    )
  }
}

private struct AccountRow: View {
  let connection: Components.Schemas.Connection
  let canReconnect: Bool
  let reconnect: () -> Void

  var body: some View {
    if connection.needsReconnect {
      Button(action: reconnect) {
        LabeledContent {
          Label("Reconnect", systemImage: "arrow.clockwise")
            .foregroundStyle(.secondary)
        } label: {
          name
        }
      }
      .tint(.primary)
      .disabled(!canReconnect)
    } else {
      LabeledContent {
        Text("Connected")
      } label: {
        name
      }
    }
  }

  private var name: some View {
    Text(connection.accountName)
      .padding(.leading, 42)
  }
}
