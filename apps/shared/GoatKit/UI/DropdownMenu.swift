import SwiftUI

/// web's `DropdownMenu`: a floating list of actions, opened with `.dropdownMenu(isPresented:)`.
struct DropdownMenu<Content: View>: View {
  @ViewBuilder let content: Content

  var body: some View {
    VStack(alignment: .leading, spacing: 0) { content }
      .padding(MenuMetrics.inset)
      .frame(minWidth: 200, alignment: .leading)
      .fixedSize()
      .background(
        Palette.popover,
        in: .rect(cornerRadius: MenuMetrics.radius, style: .continuous)
      )
      .overlay {
        RoundedRectangle(cornerRadius: MenuMetrics.radius, style: .continuous)
          .strokeBorder(Palette.border, lineWidth: 0.5)
      }
      .shadow(color: .black.opacity(0.2), radius: 20, y: 10)
  }
}

/// The menu is concentric with its items.
private enum MenuMetrics {
  static let inset: CGFloat = 4
  static let itemRadius: CGFloat = 8
  static let radius = itemRadius + inset
  static let itemShape = RoundedRectangle(cornerRadius: itemRadius, style: .continuous)
}

/// Who or what the menu is about, above its items.
struct DropdownMenuLabel: View {
  let title: String
  var subtitle: String?

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      if !title.isEmpty {
        Text(verbatim: title)
          .font(.body.weight(.semibold))
          .foregroundStyle(Palette.foreground)
      }
      if let subtitle {
        Text(verbatim: subtitle)
          .font(.callout)
          .foregroundStyle(Palette.mutedForeground)
      }
    }
    .padding(.vertical, 6)
    .padding(.leading, 8)
    .padding(.trailing, 12)
  }
}

struct DropdownMenuSeparator: View {
  var body: some View {
    Palette.border
      .frame(height: 1)
      .padding(.horizontal, 8)
      .padding(.vertical, 4)
  }
}

struct DropdownMenuItem: View {
  let title: LocalizedStringKey
  var systemImage: String?
  var shortcut: String?
  var role: ButtonRole?
  let action: () -> Void

  @State private var isHovered = false
  @Environment(\.dismissDropdownMenu) private var dismiss

  private var tint: Color { role == .destructive ? Palette.destructive : Palette.foreground }

  var body: some View {
    Button(role: role) {
      dismiss()
      action()
    } label: {
      HStack(spacing: 8) {
        if let systemImage {
          IconSlot(systemName: systemImage)
            .foregroundStyle(role == .destructive ? tint : Palette.mutedForeground)
        }
        Text(title)
          .foregroundStyle(tint)
        Spacer(minLength: 16)
        if let shortcut {
          Text(verbatim: shortcut)
            .font(.callout)
            .foregroundStyle(Palette.mutedForeground)
        }
      }
      .padding(.leading, 6)
      .padding(.trailing, 12)
      .frame(height: 32)
      .background(isHovered ? Palette.sidebarAccent : .clear, in: MenuMetrics.itemShape)
      .contentShape(MenuMetrics.itemShape)
    }
    .buttonStyle(.plain)
    .onHover { isHovered = $0 }
    .animation(.easeOut(duration: 0.12), value: isHovered)
  }
}

#Preview {
  DropdownMenu {
    DropdownMenuLabel(title: "Mino", subtitle: "mino@example.com")
    DropdownMenuSeparator()
    DropdownMenuItem(title: "Settings", systemImage: "gearshape", shortcut: "⌘,") {}
    DropdownMenuItem(title: "Sign Out", systemImage: "rectangle.portrait.and.arrow.right") {}
  }
  .padding(40)
}
