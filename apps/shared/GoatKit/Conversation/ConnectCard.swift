import AuthenticationServices
import GoatAPI
import SwiftUI

/// A service the agent asked the user to connect, in the conversation and during a call. The
/// whole card is the button; it shows connecting, connected, and a failure under it.
struct ConnectCard: View {
  let request: Components.Schemas.ConnectRequest
  let integrations: IntegrationsModel
  let agentID: String
  let session: WebAuthenticationSession

  private var id: String { request.integrationId }
  private var isConnected: Bool { integrations.isConnected(id) }
  private var isConnecting: Bool { integrations.connectingID == id }

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      Button(action: connect) {
        HStack(spacing: 10) {
          IntegrationIcon(url: integrations.iconURL(of: id), size: 28)
          Text(request.name)
            .font(.body.weight(.medium))
          Spacer(minLength: 16)
          Group {
            if isConnecting {
              ProgressView()
                .controlSize(.small)
            } else {
              Image(systemName: isConnected ? "checkmark" : "plus")
            }
          }
          .foregroundStyle(.secondary)
          .frame(width: 24)
        }
        .padding(.leading, 8)
        .padding(.trailing, 12)
        .frame(minHeight: 44)
        .background(.fill.secondary, in: .capsule)
        .contentShape(.capsule)
      }
      .buttonStyle(.plain)
      .disabled(isConnected || integrations.connectingID != nil)
      .fixedSize()
      .accessibilityLabel(isConnected ? "\(request.name), Connected" : "Connect \(request.name)")

      if integrations.failedIntegrationID == id && !isConnected {
        Text("Couldn’t connect.")
          .font(.footnote)
          .foregroundStyle(.red)
          .padding(.leading, 8)
      }
    }
    .frame(maxWidth: .infinity, alignment: .leading)
  }

  private func connect() {
    let connector = Connector(client: integrations.client, session: session)
    Task { await integrations.connect(id, agentID: agentID, with: connector) }
  }
}

extension ConversationModel {
  /// A service the agent's latest reply asks to connect that is not connected yet, for the call
  /// screen, where earlier messages are out of sight.
  func pendingConnect(in integrations: IntegrationsModel) -> Components.Schemas.ConnectRequest? {
    messages.last { $0.role.value1 == .assistant }?.connect?
      .first { !integrations.isConnected($0.integrationId) }
  }
}
