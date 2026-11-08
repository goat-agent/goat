import Foundation

extension URL {
  static let goatServer: URL = {
    guard let value = Bundle.main.object(forInfoDictionaryKey: "GoatServerURL") as? String,
      let url = URL(string: value)
    else { fatalError("Set GOAT_SERVER_URL in the build settings.") }
    return url
  }()
}
