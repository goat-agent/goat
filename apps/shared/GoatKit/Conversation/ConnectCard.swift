import SwiftUI

struct ConnectCard: View {
  let name: String
  let iconURL: String?
  let isConnected: Bool
  let isConnecting: Bool
  let isEnabled: Bool
  let didFail: Bool
  let connect: () -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      if isConnected {
        row { Label("Connected", systemImage: "checkmark").foregroundStyle(.secondary) }
      } else {
        Button(action: connect) {
          row {
            if isConnecting {
              ProgressView()
            } else {
              Image(systemName: "plus").foregroundStyle(.secondary)
            }
          }
        }
        .buttonStyle(.plain)
        .disabled(isConnecting || !isEnabled)
        .accessibilityLabel("Connect \(name)")
      }
      if didFail && !isConnected {
        Label("Couldn’t connect. Try again.", systemImage: "exclamationmark.circle.fill")
          .font(.footnote)
          .foregroundStyle(.red)
      }
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 10)
    .background(.fill.secondary, in: .rect(cornerRadius: 20))
    .frame(maxWidth: .infinity, alignment: .leading)
    .padding(.trailing, 48)
  }

  private func row(@ViewBuilder trailing: () -> some View) -> some View {
    HStack(spacing: 10) {
      IntegrationIcon(url: iconURL, size: 26)
      Text(name)
        .font(.body.weight(.medium))
      trailing()
    }
    .contentShape(.rect)
  }
}
