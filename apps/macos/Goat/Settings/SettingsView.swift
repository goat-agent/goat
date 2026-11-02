import SwiftUI

/// Goat's Settings window, laid out like the main window: the pages on the window, and the open
/// page on a card beside them.
struct SettingsView: View {
  let model: AppModel
  @Bindable var computer: ComputerService
  @State private var integrations: IntegrationsModel
  @State private var devices: DevicesModel
  @State private var page = Page.account
  @FocusState private var isPageListFocused: Bool

  private enum Page: CaseIterable, Identifiable {
    case account
    case integrations
    case devices
    case thisMac

    var id: Self { self }

    var title: LocalizedStringKey {
      switch self {
      case .account: "Account"
      case .integrations: "Integrations"
      case .devices: "Devices"
      case .thisMac: "This Mac"
      }
    }

    var systemImage: String {
      switch self {
      case .account: "person.crop.circle"
      case .integrations: "puzzlepiece.extension"
      case .devices: "iphone"
      case .thisMac: "desktopcomputer"
      }
    }
  }

  init(model: AppModel, computer: ComputerService) {
    self.model = model
    self.computer = computer
    _integrations = State(initialValue: IntegrationsModel(client: model.client))
    _devices = State(initialValue: DevicesModel(client: model.client))
  }

  var body: some View {
    HStack(spacing: 0) {
      VStack(alignment: .leading, spacing: 2) {
        ForEach(Page.allCases) { item in
          PageRow(title: item.title, systemImage: item.systemImage, isSelected: item == page) {
            page = item
          }
        }
      }
      .padding(.top, 40)
      .padding(.horizontal, 8)
      .frame(width: 196, alignment: .topLeading)
      .frame(maxHeight: .infinity, alignment: .top)
      // The window opens with the pages focused, as System Settings does, rather than with the
      // first text field selected; the arrow keys move between them.
      .focusable()
      .focusEffectDisabled()
      .focused($isPageListFocused)
      .onMoveCommand(perform: movePage)

      ScrollView {
        content
          .padding(24)
          .frame(maxWidth: .infinity, alignment: .leading)
      }
      .scrollIndicators(.never)
      .background(Palette.background, in: .rect(cornerRadius: 12, style: .continuous))
      .padding([.top, .trailing, .bottom], 8)
    }
    .background(Palette.sidebar)
    .ignoresSafeArea(.container, edges: .top)
    .frame(width: 760, height: 520)
    .defaultFocus($isPageListFocused, true)
    .portalHost()
    .toolbar(removing: .title)
    .toolbarBackgroundVisibility(.hidden, for: .windowToolbar)
  }

  private func movePage(_ direction: MoveCommandDirection) {
    let pages = Page.allCases
    guard let index = pages.firstIndex(of: page) else { return }
    switch direction {
    case .up where index > pages.startIndex: page = pages[index - 1]
    case .down where index < pages.endIndex - 1: page = pages[index + 1]
    default: break
    }
  }

  @ViewBuilder private var content: some View {
    switch page {
    case .account: AccountSection(model: model)
    case .integrations: IntegrationsSection(integrations: integrations)
    case .devices: DevicesSection(devices: devices)
    case .thisMac: ThisMacSection(computer: computer)
    }
  }
}

/// Whether the agent may use this Mac, and the two permissions that takes.
private struct ThisMacSection: View {
  @Bindable var computer: ComputerService

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      Text("This Mac")
        .font(.sectionTitle)
        .foregroundStyle(Palette.foreground)
      ItemGroup {
        Switch(isOn: $computer.isEnabled) {
          VStack(alignment: .leading, spacing: 1) {
            Text("Let the agent use this Mac")
              .font(.body.weight(.medium))
              .foregroundStyle(Palette.foreground)
            Text("It sees the screen, clicks, and types. Press Esc to stop it.")
              .font(.callout)
              .foregroundStyle(Palette.mutedForeground)
          }
        }
        .disabled(!computer.permissions.areGranted)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
      }
      Text("Permissions")
        .font(.callout.weight(.semibold))
        .foregroundStyle(Palette.mutedForeground)
        .padding(.top, 8)
        .padding(.leading, 12)
      PermissionRows(permissions: computer.permissions)
    }
    .task { await computer.permissions.watch() }
  }
}
