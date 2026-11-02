import AuthenticationServices
import GoatAPI
import SwiftUI

/// The connected accounts, one row each, and the way to add another. Settings on both devices
/// shows this section.
struct IntegrationsSection: View {
  let integrations: IntegrationsModel
  @State private var isAdding = false
  @Environment(\.webAuthenticationSession) private var webAuthenticationSession

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      // The button rides on the title's line, so the title sits where every other page's does.
      Text("Integrations")
        .font(.sectionTitle)
        .foregroundStyle(Palette.foreground)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .trailing) {
          Button("Add Integration", systemImage: "plus") { isAdding = true }
            .buttonStyle(.secondary)
            .controlSize(.small)
        }
      if !integrations.accounts.isEmpty {
        ItemGroup {
          ForEach(integrations.accounts) { account in
            AccountRow(account: account, canReconnect: integrations.connectingID == nil) {
              let connector = Connector(
                client: integrations.client, session: webAuthenticationSession)
              Task { await integrations.connect(account.integration.id, with: connector) }
            } disconnect: {
              Task {
                await integrations.disconnect(account.connection, of: account.integration.id)
              }
            }
          }
        }
      }
    }
    .sheet(isPresented: $isAdding) {
      IntegrationCatalogView(integrations: integrations)
    }
    .task { await integrations.load() }
  }
}

/// A connected account: the service, which of the user's accounts it is, and what can be done.
private struct AccountRow: View {
  let account: IntegrationsModel.Account
  let canReconnect: Bool
  let reconnect: () -> Void
  let disconnect: () -> Void

  @State private var isShowingMenu = false
  @State private var isConfirmingDisconnect = false

  var body: some View {
    Item(title: account.integration.name, description: account.connection.accountName) {
      IntegrationIcon(url: account.integration.iconUrl, size: IconSlot.Size.item.side)
    } trailing: {
      if account.connection.needsReconnect {
        Button("Reconnect", action: reconnect)
          .buttonStyle(.primary)
          .controlSize(.small)
          .disabled(!canReconnect)
          .accessibilityLabel("Reconnect \(account.connection.accountName)")
      }
      Button("More", systemImage: "ellipsis") { isShowingMenu = true }
        .buttonStyle(.icon)
        .dropdownMenu(isPresented: $isShowingMenu, placement: .below(.trailing)) {
          DropdownMenuItem(title: "Disconnect", role: .destructive) {
            isConfirmingDisconnect = true
          }
        }
    }
    .alertDialog(
      "Disconnect \(account.integration.name)?", isPresented: $isConfirmingDisconnect,
      message: "Agents can no longer use \(account.connection.accountName).", action: "Disconnect",
      perform: disconnect)
  }
}
