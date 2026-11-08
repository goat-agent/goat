import Foundation

public enum ComputerError: Error, LocalizedError {
  case windowGone
  case minimized
  case outsideWindow
  case appNotFound(String)
  case noWindowOpened
  case unknownKey(String)
  case captureFailed

  public var errorDescription: String? {
    switch self {
    case .windowGone: "The window is gone. List the windows again."
    case .minimized:
      "The window is minimized. An action with foreground: true restores it after asking the user."
    case .outsideWindow: "The point is outside the window's image."
    case .appNotFound(let name): "No app named \(name) is installed."
    case .noWindowOpened: "Nothing opened a window."
    case .unknownKey(let keys): "\(keys) is not a key this computer knows."
    case .captureFailed: "The window could not be captured."
    }
  }
}
