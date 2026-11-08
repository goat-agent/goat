import Foundation
import Security

/// Strings kept in the Keychain for this device only, so a backup cannot carry them elsewhere.
nonisolated enum Keychain {
  static func read(_ account: String) -> String? {
    var query = query(account)
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne
    var item: CFTypeRef?
    guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
      let data = item as? Data
    else { return nil }
    return String(data: data, encoding: .utf8)
  }

  static func write(_ value: String, for account: String) {
    delete(account)
    var query = query(account)
    query[kSecValueData as String] = Data(value.utf8)
    query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    SecItemAdd(query as CFDictionary, nil)
  }

  static func delete(_ account: String) {
    SecItemDelete(query(account) as CFDictionary)
  }

  private static func query(_ account: String) -> [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: Bundle.main.bundleIdentifier ?? "Goat",
      kSecAttrAccount as String: account,
    ]
  }
}
