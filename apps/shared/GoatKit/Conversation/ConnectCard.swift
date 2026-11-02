import AuthenticationServices
import GoatAPI
import SwiftUI

/// A service the agent asked the user to connect, in the conversation and during a call: the
/// service, and its one next action. It shows connecting, connected, and a failure under it.
struct ConnectCard: View {
  let request: Components.Schemas.ConnectRequest
  let integrations: IntegrationsModel
  let agentID: String
  let session: WebAuthenticationSession

  private var id: String { request.integrationId }
  private var isConnected: Bool { integrations.isConnected(id) }
  private var isConnecting: Bool { integrations.connectingID == id }

  /// Concentric with its small button.
  private static let inset: CGFloat = 10
  private static let radius = ControlSize.small.height / 2 + inset

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      HStack(spacing: 10) {
        IntegrationIcon(url: integrations.iconURL(of: id), size: 32)
        Text(request.name)
          .font(.body.weight(.medium))
          .foregroundStyle(Palette.foreground)
        Spacer(minLength: 16)
        if isConnecting {
          ProgressView()
            .controlSize(.small)
        } else if isConnected {
          Label("Connected", systemImage: "checkmark")
            .font(.callout)
            .foregroundStyle(Palette.mutedForeground)
        } else {
          Button("Connect", action: connect)
            .buttonStyle(.primary)
            .controlSize(.small)
            .disabled(integrations.connectingID != nil)
            .accessibilityLabel("Connect \(request.name)")
        }
      }
      .padding(Self.inset)
      .frame(maxWidth: 320)
      .overlay {
        RoundedRectangle(cornerRadius: Self.radius, style: .continuous)
          .strokeBorder(Palette.border)
      }

      if integrations.failedIntegrationID == id && !isConnected {
        Text("Couldn’t connect.")
          .font(.footnote)
          .foregroundStyle(Palette.destructive)
          .padding(.leading, 12)
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
