import Foundation

/// What went wrong, worded for the agent: it reads these and decides what to do next.
public enum ComputerError: Error, LocalizedError {
  case notAllowed
  case windowGone
  case noDisplay
  case unknownRef
  case notAnElement
  case appNotFound(String)
  case menuNotFound(String)
  case cannotSet(String)
  case badKeys(String)
  case nothingToOpen
  case stopped

  public var errorDescription: String? {
    switch self {
    case .notAllowed:
      "The user has not allowed the agent to use this Mac."
    case .windowGone:
      "That window is closed now. Look at the screen again."
    case .noDisplay:
      "No display is available to look at."
    case .unknownRef:
      "That id is not on this Mac now. Look again."
    case .notAnElement:
      "That needs an element id from the window's element list."
    case .appNotFound(let name):
      "No app named \(name) is installed."
    case .menuNotFound(let item):
      "The menu has no item named \(item)."
    case .cannotSet(let reason):
      reason
    case .badKeys(let keys):
      "\(keys) is not a key combination this Mac knows."
    case .nothingToOpen:
      "Give an app, a path, or a URL to open."
    case .stopped:
      "The user stopped the agent from using this Mac."
    }
  }
}
