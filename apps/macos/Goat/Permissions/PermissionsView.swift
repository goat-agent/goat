import SwiftUI

/// Asks once, after the first sign-in on this Mac, whether the agent may use it, and walks the
/// user through the two system permissions that takes.
struct PermissionsView: View {
  let computer: ComputerService
  @Environment(\.dismissWindow) private var dismissWindow

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      Text("Let the agent use this Mac?")
        .font(.system(size: 20, weight: .bold))
        .foregroundStyle(Palette.foreground)
      Text("Turn on these two. You can stop it from the menu bar at any time.")
        .foregroundStyle(Palette.mutedForeground)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.top, 4)

      PermissionRows(permissions: computer.permissions)
        .padding(.top, 18)

      Text(
        "Turn Goat on in System Settings and come back; this updates by itself. After allowing screen recording, quit and reopen Goat."
      )
      .font(.footnote)
      .foregroundStyle(Palette.mutedForeground)
      .fixedSize(horizontal: false, vertical: true)
      .padding(.top, 10)
      .padding(.horizontal, 12)

      Spacer(minLength: 24)

      HStack(spacing: 8) {
        Spacer()
        Button("Not Now") { dismissWindow() }
          .buttonStyle(.secondary)
          .keyboardShortcut(.cancelAction)
        Button("Done") {
          computer.isEnabled = true
          dismissWindow()
        }
        .buttonStyle(.primary)
        .keyboardShortcut(.defaultAction)
        .disabled(!computer.permissions.areGranted)
      }
    }
    .padding(.top, 44)
    .padding([.horizontal, .bottom], 28)
    .frame(width: 460, height: 400)
    .containerBackground(Palette.background, for: .window)
    .toolbar(removing: .title)
    .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
    .task { await computer.permissions.watch() }
  }
}

/// The two permissions, each with its state or a way to grant it.
struct PermissionRows: View {
  let permissions: ComputerPermissions

  var body: some View {
    ItemGroup {
      row(
        String(localized: "Accessibility"), systemImage: "accessibility",
        isGranted: permissions.canUseAccessibility, kind: .accessibility)
      row(
        String(localized: "Screen & System Audio Recording"),
        systemImage: "rectangle.dashed.badge.record", isGranted: permissions.canRecordScreen,
        kind: .screenRecording)
    }
  }

  private func row(
    _ title: String, systemImage: String, isGranted: Bool, kind: ComputerPermissions.Kind
  ) -> some View {
    Item(title: title) {
      IconSlot(systemName: systemImage, size: .item)
        .foregroundStyle(Palette.mutedForeground)
    } trailing: {
      if isGranted {
        Label("On", systemImage: "checkmark")
          .font(.callout)
          .foregroundStyle(Palette.mutedForeground)
      } else {
        Button("Open System Settings") { permissions.request(kind) }
          .buttonStyle(.secondary)
          .controlSize(.small)
      }
    }
  }
}
