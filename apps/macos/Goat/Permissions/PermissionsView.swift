import SwiftUI

/// Asks once, after the first sign-in on this Mac, whether the agent may use it, and walks the
/// user through the two system permissions that takes.
struct PermissionsView: View {
  let computer: ComputerService
  @Environment(\.dismissWindow) private var dismissWindow

  var body: some View {
    VStack(spacing: 0) {
      AgentFace(size: 64)
      Text("Let the agent use this Mac?")
        .font(.title2.bold())
        .padding(.top, 16)
      Text(
        "Then what you ask from your iPhone can be done here, with this Mac’s apps and files. The menu bar shows when the agent is using it, and you can stop it any time."
      )
      .multilineTextAlignment(.center)
      .foregroundStyle(.secondary)
      .padding(.top, 8)

      PermissionRows(permissions: computer.permissions)
        .padding(.top, 24)

      Text(
        "Turn Goat on in System Settings and come back; this updates by itself. After allowing screen recording, quit and reopen Goat."
      )
      .font(.footnote)
      .foregroundStyle(.tertiary)
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(.top, 10)

      Spacer(minLength: 24)

      Button {
        computer.isEnabled = true
        dismissWindow()
      } label: {
        Text("Done").frame(maxWidth: .infinity)
      }
      .buttonStyle(.glassProminent)
      .controlSize(.large)
      .disabled(!computer.permissions.areGranted)

      Button("Not Now") { dismissWindow() }
        .buttonStyle(.borderless)
        .padding(.top, 10)
    }
    .padding(36)
    .frame(width: 480, height: 520)
    .task { await computer.permissions.watch() }
  }
}

/// The two permissions, each with its state or a way to grant it.
struct PermissionRows: View {
  let permissions: ComputerPermissions

  var body: some View {
    VStack(spacing: 0) {
      row(
        "Accessibility", detail: "Press buttons and type for you.", systemImage: "accessibility",
        color: .blue, isGranted: permissions.canUseAccessibility, kind: .accessibility)
      Divider()
      row(
        "Screen & System Audio Recording", detail: "See app windows to know what is on them.",
        systemImage: "rectangle.dashed.badge.record", color: .pink,
        isGranted: permissions.canRecordScreen, kind: .screenRecording)
    }
    .background(.fill.quinary, in: .rect(cornerRadius: 12))
  }

  private func row(
    _ title: LocalizedStringKey, detail: LocalizedStringKey, systemImage: String, color: Color,
    isGranted: Bool, kind: ComputerPermissions.Kind
  ) -> some View {
    HStack(spacing: 12) {
      Image(systemName: systemImage)
        .font(.body.weight(.semibold))
        .foregroundStyle(.white)
        .frame(width: 30, height: 30)
        .background(color, in: .rect(cornerRadius: 7))
      VStack(alignment: .leading, spacing: 2) {
        Text(title).font(.body.weight(.medium))
        Text(detail).font(.caption).foregroundStyle(.secondary)
      }
      Spacer()
      if isGranted {
        Label("On", systemImage: "checkmark.circle.fill")
          .foregroundStyle(.green)
      } else {
        Button("Open System Settings") { permissions.request(kind) }
      }
    }
    .padding(12)
  }
}
