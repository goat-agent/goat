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
              Button("연결 끊기", role: .destructive) {
                Task { await model.disconnect(connection, of: integration.id) }
              }
            }
          }
        }
      } footer: {
        if let failed = model.integrations.first(where: { $0.id == model.failedIntegrationID }) {
          FormFooter(hint: "", error: "\(failed.name) 연결에 실패했어요. 잠시 후 다시 시도해 주세요.")
        }
      }
    }
    .navigationTitle("연동")
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
      integration.connections.isEmpty ? "\(integration.name) 연결" : "\(integration.name) 계정 추가"
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
          Label("다시 연결", systemImage: "arrow.clockwise")
            .foregroundStyle(.secondary)
        } label: {
          name
        }
      }
      .tint(.primary)
      .disabled(!canReconnect)
    } else {
      LabeledContent {
        Text("연결됨")
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
