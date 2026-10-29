import Foundation
import Security

nonisolated struct TokenStore: Sendable {
  private var query: [String: Any] {
    [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: Bundle.main.bundleIdentifier ?? "Goat",
      kSecAttrAccount as String: "session-token",
    ]
  }

  func load() -> String? {
    var query = query
    query[kSecReturnData as String] = true
    query[kSecMatchLimit as String] = kSecMatchLimitOne

    var item: CFTypeRef?
    guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
      let data = item as? Data
    else { return nil }
    return String(data: data, encoding: .utf8)
  }

  func save(_ token: String) {
    clear()
    var query = query
    query[kSecValueData as String] = Data(token.utf8)
    query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
    SecItemAdd(query as CFDictionary, nil)
  }

  func clear() {
    SecItemDelete(query as CFDictionary)
  }
}
