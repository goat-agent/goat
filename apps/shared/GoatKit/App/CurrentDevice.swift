import Foundation
import GoatAPI
import Security

#if os(iOS)
  import UIKit
#endif

/// This app install as the server knows it. The install id is made once and kept in the
/// Keychain as "this device only", so a backup restored onto another device cannot carry it there
/// and make two devices one.
enum CurrentDevice {
  static var signingIn: Components.Schemas.SigningInDevice {
    #if os(iOS)
      .init(
        installationId: installationID, name: UIDevice.current.name, platform: .init(value1: .ios))
    #else
      .init(
        installationId: installationID,
        name: Host.current().localizedName ?? "Mac",
        platform: .init(value1: .macos)
      )
    #endif
  }

  private static let installationID: String = {
    let query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: Bundle.main.bundleIdentifier ?? "Goat",
      kSecAttrAccount as String: "installation-id",
    ]
    var lookup = query
    lookup[kSecReturnData as String] = true
    lookup[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    if SecItemCopyMatching(lookup as CFDictionary, &item) == errSecSuccess,
      let data = item as? Data, let id = String(data: data, encoding: .utf8)
    {
      return id
    }

    let id = UUID().uuidString.lowercased()
    var entry = query
    entry[kSecValueData as String] = Data(id.utf8)
    entry[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    SecItemAdd(entry as CFDictionary, nil)
    return id
  }()
}
