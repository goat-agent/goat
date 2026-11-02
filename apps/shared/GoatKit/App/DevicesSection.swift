import GoatAPI
import SwiftUI

/// Where the account is signed in. Removing a device signs it out; this one signs out from the
/// account section instead.
struct DevicesSection: View {
  let devices: DevicesModel

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      Text("Devices")
        .font(.sectionTitle)
        .foregroundStyle(Palette.foreground)
      if !devices.devices.isEmpty {
        ItemGroup {
          ForEach(devices.devices, id: \.id) { device in
            DeviceRow(device: device) {
              Task { await devices.remove(device.id) }
            }
          }
        }
      }
    }
    .task { await devices.load() }
  }
}

private struct DeviceRow: View {
  let device: Components.Schemas.Device
  let remove: () -> Void

  private var systemImage: String {
    switch device.platform.value1 {
    case .ios: "iphone"
    case .macos: "laptopcomputer"
    default: "globe"
    }
  }

  private var detail: String {
    device.isCurrent
      ? String(localized: "This device")
      : String(
        localized: "Signed in \(device.createdAt.formatted(date: .abbreviated, time: .omitted))")
  }

  var body: some View {
    Item(title: device.name, description: detail) {
      IconSlot(systemName: systemImage, size: .item)
        .foregroundStyle(Palette.mutedForeground)
    } trailing: {
      if !device.isCurrent {
        Button("Remove", action: remove)
          .buttonStyle(.secondary)
          .controlSize(.small)
      }
    }
  }
}
