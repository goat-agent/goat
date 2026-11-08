import SwiftUI

struct MenuBarContent: View {
  let model: AppModel

  var body: some View {
    VStack(alignment: .leading, spacing: 12) {
      if model.account.isSignedIn {
        SignedIn(model: model)
      } else {
        SignInView(account: model.account, onSignIn: model.didSignIn)
      }
      Divider()
      Button("Quit Goat") { NSApp.terminate(nil) }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
    }
    .padding(16)
    .frame(width: 320)
  }
}

private struct SignedIn: View {
  let model: AppModel

  var body: some View {
    @Bindable var computerUse = model.computerUse
    VStack(alignment: .leading, spacing: 12) {
      Toggle("Let the agent use this Mac", isOn: $computerUse.isEnabled)
        .toggleStyle(.switch)
      Text(status)
        .font(.callout)
        .foregroundStyle(.secondary)
      if !model.permissions.areGranted {
        PermissionsList(permissions: model.permissions)
      }
      Button("Sign Out") { Task { await model.signOut() } }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
    }
  }

  private var status: LocalizedStringKey {
    switch model.connection.state {
    case .online: model.computerUse.isActing ? "Working on this Mac…" : "Connected"
    case .connecting: "Connecting…"
    case .offline: "Offline"
    }
  }
}

private struct PermissionsList: View {
  let permissions: Permissions

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      Text("Goat needs two permissions to see and use your apps.")
        .font(.callout)
      row("Accessibility", isGranted: permissions.hasAccessibility, kind: .accessibility)
      row(
        "Screen & System Audio Recording", isGranted: permissions.hasScreenRecording,
        kind: .screenRecording)
    }
  }

  private func row(_ title: LocalizedStringKey, isGranted: Bool, kind: Permissions.Kind)
    -> some View
  {
    HStack {
      Text(title)
      Spacer()
      if isGranted {
        Image(systemName: "checkmark")
          .foregroundStyle(.secondary)
          .accessibilityLabel("Allowed")
      } else {
        Button("Allow…") { permissions.request(kind) }
      }
    }
  }
}
