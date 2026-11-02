import SwiftUI

extension View {
  /// web's `AlertDialog`: asks before something that cannot be undone, drawn in the window's
  /// portal. Escape cancels and Return confirms, as their keyboard shortcuts.
  func alertDialog(
    _ title: LocalizedStringKey, isPresented: Binding<Bool>, message: LocalizedStringKey,
    action: LocalizedStringKey, perform: @escaping () -> Void
  ) -> some View {
    modifier(
      AlertDialog(
        title: title, message: message, action: action, isPresented: isPresented, perform: perform))
  }
}

private struct AlertDialog: ViewModifier {
  let title: LocalizedStringKey
  let message: LocalizedStringKey
  let action: LocalizedStringKey
  @Binding var isPresented: Bool
  let perform: () -> Void

  func body(content: Content) -> some View {
    content.portalDialog(isPresented: $isPresented) {
      AlertDialogCard(title: title, message: message, action: action) {
        isPresented = false
      } perform: {
        isPresented = false
        perform()
      }
    }
  }
}

private struct AlertDialogCard: View {
  let title: LocalizedStringKey
  let message: LocalizedStringKey
  let action: LocalizedStringKey
  let cancel: () -> Void
  let perform: () -> Void

  @Environment(\.controlSize) private var controlSize

  /// Concentric with the buttons it holds.
  private static let inset: CGFloat = 10

  var body: some View {
    VStack(alignment: .leading, spacing: 6) {
      Text(title)
        .font(.headline)
        .foregroundStyle(Palette.foreground)
      Text(message)
        .foregroundStyle(Palette.mutedForeground)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.bottom, 12)
      HStack(spacing: 8) {
        Button(action: cancel) {
          Text("Cancel").frame(maxWidth: .infinity)
        }
        .buttonStyle(.secondary)
        .keyboardShortcut(.cancelAction)
        Button(action: perform) {
          Text(action).frame(maxWidth: .infinity)
        }
        .buttonStyle(.destructive)
        .keyboardShortcut(.defaultAction)
      }
    }
    .padding([.top, .horizontal], 22)
    .padding(.bottom, Self.inset)
    .frame(width: 300)
    .background(
      Palette.popover,
      in: .rect(cornerRadius: controlSize.height / 2 + Self.inset, style: .continuous)
    )
    .shadow(color: .black.opacity(0.2), radius: 24, y: 12)
  }
}

#Preview {
  @Previewable @State var isPresented = true
  Color.clear
    .frame(width: 480, height: 320)
    .alertDialog(
      "Sign out?", isPresented: $isPresented,
      message: "To sign back in, you’ll need a code from your email.", action: "Sign Out"
    ) {}
    .portalHost()
}
