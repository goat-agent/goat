import CoreGraphics

/// Something to do in a window. Points are pixels of the window's latest full image.
public enum Action: Sendable, Equatable {
  case click(at: CGPoint, button: MouseButton, count: Int, modifiers: Modifiers)
  case drag(from: CGPoint, to: CGPoint, modifiers: Modifiers)
  case scroll(at: CGPoint, direction: ScrollDirection, amount: Int)
  case move(to: CGPoint)
  case type(String)
  case key(KeyCombo, repeat: Int)

  var isPointer: Bool {
    switch self {
    case .click, .drag, .scroll, .move: true
    case .type, .key: false
    }
  }

  func mapped(_ transform: (CGPoint) throws -> CGPoint) rethrows -> Action {
    switch self {
    case .click(let at, let button, let count, let modifiers):
      .click(at: try transform(at), button: button, count: count, modifiers: modifiers)
    case .drag(let from, let to, let modifiers):
      .drag(from: try transform(from), to: try transform(to), modifiers: modifiers)
    case .scroll(let at, let direction, let amount):
      .scroll(at: try transform(at), direction: direction, amount: amount)
    case .move(let to):
      .move(to: try transform(to))
    case .type, .key:
      self
    }
  }
}

public enum MouseButton: Sendable, Equatable {
  case left, right, middle
}

public enum ScrollDirection: Sendable, Equatable {
  case up, down, left, right
}

public struct Modifiers: OptionSet, Sendable, Hashable {
  public let rawValue: Int

  public init(rawValue: Int) {
    self.rawValue = rawValue
  }

  public static let command = Modifiers(rawValue: 1 << 0)
  public static let shift = Modifiers(rawValue: 1 << 1)
  public static let option = Modifiers(rawValue: 1 << 2)
  public static let control = Modifiers(rawValue: 1 << 3)

  var flags: CGEventFlags {
    var flags: CGEventFlags = []
    if contains(.command) { flags.insert(.maskCommand) }
    if contains(.shift) { flags.insert(.maskShift) }
    if contains(.option) { flags.insert(.maskAlternate) }
    if contains(.control) { flags.insert(.maskControl) }
    return flags
  }
}
