import Foundation
import GoatAPI

/// This app install as the server knows it.
enum Installation {
  static var device: Components.Schemas.SigningInDevice {
    .init(
      installationId: id, name: Host.current().localizedName ?? "Mac",
      platform: .init(value1: .macos))
  }

  private static let id: String = {
    if let id = Keychain.read("installation-id") { return id }
    let id = UUID().uuidString.lowercased()
    Keychain.write(id, for: "installation-id")
    return id
  }()
}
