import SwiftUI

/// One page in the list on the left of a settings window or dialog.
struct PageRow: View {
  let title: LocalizedStringKey
  let systemImage: String
  let isSelected: Bool
  let select: () -> Void

  @State private var isHovered = false

  var body: some View {
    Button(action: select) {
      HStack(spacing: 8) {
        IconSlot(systemName: systemImage)
          .foregroundStyle(Palette.mutedForeground)
        Text(title)
          .fontWeight(isSelected ? .medium : .regular)
          .foregroundStyle(Palette.foreground)
      }
      .padding(.horizontal, 6)
      .frame(maxWidth: .infinity, minHeight: 32, alignment: .leading)
      .background(
        isSelected ? Palette.sidebarAccent : isHovered ? Palette.accent : .clear,
        in: .rect(cornerRadius: 8, style: .continuous)
      )
      .contentShape(.rect(cornerRadius: 8, style: .continuous))
    }
    .buttonStyle(.plain)
    .onHover { isHovered = $0 }
    .accessibilityAddTraits(isSelected ? .isSelected : [])
  }
}
