import SwiftUI

/// The one next action of a screen, meant for a bottom `safeAreaBar` so it stays above the keyboard.
struct PrimaryButton: View {
  let title: LocalizedStringKey
  let workingTitle: LocalizedStringKey
  let isWorking: Bool
  let isEnabled: Bool
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      HStack(spacing: 8) {
        if isWorking {
          ProgressView()
            .tint(Palette.mutedForeground)
        }
        Text(isWorking ? workingTitle : title)
      }
      .frame(maxWidth: .infinity)
    }
    .buttonStyle(.primary)
    .controlSize(.large)
    .disabled(!isEnabled || isWorking)
    .padding(.horizontal)
    .padding(.bottom, 8)
  }
}
