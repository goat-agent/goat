import Foundation
import GoatAPI
import Observation

/// The devices signed in to the account: this one, the user's other apps, and browsers.
@Observable
final class DevicesModel {
  private(set) var devices: [Components.Schemas.Device] = []

  private let client: Client

  init(client: Client) {
    self.client = client
  }

  func load() async {
    guard case .ok(let response) = try? await client.listDevices(),
      let list = try? response.body.json.devices
    else { return }
    devices = list
  }

  /// Signs that device out.
  func remove(_ deviceID: String) async {
    guard case .noContent = try? await client.removeDevice(path: .init(id: deviceID)) else {
      return
    }
    devices.removeAll { $0.id == deviceID }
  }
}
