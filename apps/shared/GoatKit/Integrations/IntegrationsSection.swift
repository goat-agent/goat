import AuthenticationServices
import GoatAPI
import SwiftUI

/// The connected accounts, one row each, and the way to add another. Settings on both devices
/// shows this section; it needs a navigation stack for the catalog.
struct IntegrationsSection: View {
  let integrations: IntegrationsModel
  @Environment(\.webAuthenticationSession) private var webAuthenticationSession

  var body: some View {
    Section("Integrations") {
      ForEach(integrations.accounts) { account in
        AccountRow(account: account, canReconnect: integrations.connectingID == nil) {
          let connector = Connector(client: integrations.client, session: webAuthenticationSession)
          Task { await integrations.connect(account.integration.id, with: connector) }
        }
        .swipeActions { disconnectButton(account) }
        .contextMenu { disconnectButton(account) }
      }
      NavigationLink {
        IntegrationCatalogView(integrations: integrations)
      } label: {
        Label {
          Text("Add Integration")
        } icon: {
          Image(systemName: "plus")
            .foregroundStyle(.secondary)
        }
      }
    }
    .task { await integrations.load() }
  }

  private func disconnectButton(_ account: IntegrationsModel.Account) -> some View {
    Button("Disconnect", systemImage: "minus.circle", role: .destructive) {
      Task { await integrations.disconnect(account.connection, of: account.integration.id) }
    }
  }
}

/// A connected account: its own name first, since one service may hold several accounts.
private struct AccountRow: View {
  let account: IntegrationsModel.Account
  let canReconnect: Bool
  let reconnect: () -> Void

  var body: some View {
    if account.connection.needsReconnect {
      Button(action: reconnect) {
        HStack {
          details
          Spacer()
          Image(systemName: "arrow.clockwise")
            .foregroundStyle(.secondary)
        }
      }
      .tint(.primary)
      .disabled(!canReconnect)
      .accessibilityLabel("Reconnect \(account.connection.accountName)")
    } else {
      details
    }
  }

  private var details: some View {
    HStack(spacing: 12) {
      IntegrationIcon(url: account.integration.iconUrl, size: 28)
      VStack(alignment: .leading, spacing: 0) {
        Text(account.connection.accountName)
        Text(account.integration.name)
          .font(.footnote)
          .foregroundStyle(.secondary)
      }
    }
  }
}
