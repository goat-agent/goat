import Foundation

public struct WindowSummary: Sendable {
  public let ref: String
  public let app: String
  public let title: String
  public let isFront: Bool
  public let isMinimized: Bool
}

public struct View: Sendable {
  public let window: WindowSummary
  public let jpeg: Data
  public let width: Int
  public let height: Int
}

public enum OpenTarget: Sendable {
  case app(String)
  case file(String)
  case url(URL)
}
