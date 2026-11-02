import SwiftUI

/// Shown while the server is out of reach; it goes away by itself once the app is back.
struct ConnectingBanner: View {
  var body: some View {
    HStack(spacing: 8) {
      ProgressView()
        .controlSize(.mini)
      Text("Connecting…")
    }
    .font(.callout)
    .foregroundStyle(Palette.mutedForeground)
    .padding(.horizontal, 12)
    .frame(height: 28)
    .background(Palette.muted, in: .capsule)
  }
}
