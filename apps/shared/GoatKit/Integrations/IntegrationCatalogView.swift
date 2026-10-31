import AuthenticationServices
import GoatAPI
import SwiftUI

/// Every service goat can connect. Picking one already connected adds another account.
struct IntegrationCatalogView: View {
  let integrations: IntegrationsModel
  @State private var query = ""
  @Environment(\.dismiss) private var dismiss
  @Environment(\.webAuthenticationSession) private var webAuthenticationSession

  private var shown: [Components.Schemas.Integration] {
    let query = query.trimmingCharacters(in: .whitespaces)
    let all = integrations.integrations.sorted {
      $0.name.localizedStandardCompare($1.name) == .orderedAscending
    }
    return query.isEmpty ? all : all.filter { $0.name.localizedStandardContains(query) }
  }

  var body: some View {
    List {
      Section {
        ForEach(shown, id: \.id) { integration in
          Button {
            connect(integration.id)
          } label: {
            HStack(spacing: 12) {
              IntegrationIcon(url: integration.iconUrl, size: 28)
              Text(integration.name)
              Spacer()
              if integrations.connectingID == integration.id {
                ProgressView()
              }
            }
          }
          .tint(.primary)
          .disabled(integrations.connectingID != nil)
        }
      } footer: {
        if let failed = integrations.integrations.first(where: {
          $0.id == integrations.failedIntegrationID
        }) {
          FormFooter(hint: "", error: String(localized: "Couldn’t connect \(failed.name)."))
        }
      }
    }
    .navigationTitle("Add Integration")
    .searchable(text: $query)
  }

  private func connect(_ integrationID: String) {
    let connector = Connector(client: integrations.client, session: webAuthenticationSession)
    Task {
      let before = integrations.accounts.count
      await integrations.connect(integrationID, with: connector)
      if integrations.accounts.count > before { dismiss() }
    }
  }
}
