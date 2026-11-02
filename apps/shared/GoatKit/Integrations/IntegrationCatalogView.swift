import AuthenticationServices
import GoatAPI
import SwiftUI

/// Every service goat can connect, in a sheet. A service already connected adds another account.
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

  private var failed: Components.Schemas.Integration? {
    integrations.integrations.first { $0.id == integrations.failedIntegrationID }
  }

  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      HStack {
        Text("Add Integration")
          .font(.title3.weight(.semibold))
          .foregroundStyle(Palette.foreground)
        Spacer()
        Button("Close", systemImage: "xmark") { dismiss() }
          .buttonStyle(.icon)
          .keyboardShortcut(.cancelAction)
      }
      TextField("Search", text: $query, prompt: Text("Search"))
        .labelsHidden()
        .inputStyle()
      ScrollView {
        VStack(spacing: 2) {
          ForEach(shown, id: \.id) { integration in
            row(integration)
          }
        }
      }
      .scrollIndicators(.never)
      if let failed {
        FormFooter(hint: "", error: String(localized: "Couldn’t connect \(failed.name)."))
      }
    }
    .padding(16)
    #if os(macOS)
      .frame(width: 440, height: 520)
    #endif
    .presentationBackground(Palette.popover)
  }

  private func row(_ integration: Components.Schemas.Integration) -> some View {
    HStack(spacing: 10) {
      IntegrationIcon(url: integration.iconUrl, size: IconSlot.Size.item.side)
      Text(integration.name)
        .font(.body.weight(.medium))
        .foregroundStyle(Palette.foreground)
      Spacer()
      if integrations.connectingID == integration.id {
        ProgressView()
          .controlSize(.small)
      } else {
        Button(integrations.isConnected(integration.id) ? "Add Account" : "Connect") {
          connect(integration.id)
        }
        .buttonStyle(.secondary)
        .controlSize(.small)
        .disabled(integrations.connectingID != nil)
      }
    }
    .padding(.horizontal, 10)
    .frame(minHeight: 48)
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
