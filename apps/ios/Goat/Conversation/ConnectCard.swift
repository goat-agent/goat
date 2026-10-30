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
        row { Label("연결됨", systemImage: "checkmark").foregroundStyle(.secondary) }
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
        .accessibilityLabel("\(name) 연결")
      }
      if didFail && !isConnected {
        Label("연결하지 못했어요. 다시 시도해 주세요.", systemImage: "exclamationmark.circle.fill")
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
