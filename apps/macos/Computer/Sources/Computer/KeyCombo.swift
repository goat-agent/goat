import CoreGraphics

/// A key with its modifiers, as written in `cmd+shift+t`.
public struct KeyCombo: Sendable, Equatable {
  public let code: CGKeyCode
  /// What the key types, for keys that type something. Menus name their shortcuts by it.
  public let character: String?
  public let modifiers: Modifiers

  public init?(_ text: String) {
    var parts = text.lowercased().split(separator: "+").map {
      $0.trimmingCharacters(in: .whitespaces)
    }
    guard let last = parts.popLast(), !last.isEmpty else { return nil }
    var modifiers: Modifiers = []
    for part in parts where !part.isEmpty {
      guard let modifier = Self.modifiers[part] else { return nil }
      modifiers.insert(modifier)
    }
    if let named = Self.named[last] {
      self.init(code: named, character: nil, modifiers: modifiers)
    } else if last.count == 1, let code = Self.characters[last] {
      self.init(code: code, character: last.uppercased(), modifiers: modifiers)
    } else {
      return nil
    }
  }

  init(code: CGKeyCode, character: String?, modifiers: Modifiers) {
    self.code = code
    self.character = character
    self.modifiers = modifiers
  }

  static let returnKey = KeyCombo(code: 36, character: nil, modifiers: [])
  static let tabKey = KeyCombo(code: 48, character: nil, modifiers: [])

  private static let modifiers: [String: Modifiers] = [
    "cmd": .command, "command": .command, "⌘": .command,
    "shift": .shift, "⇧": .shift,
    "option": .option, "opt": .option, "alt": .option, "⌥": .option,
    "ctrl": .control, "control": .control, "⌃": .control,
  ]

  private static let named: [String: CGKeyCode] = [
    "return": 36, "enter": 36, "tab": 48, "space": 49, "delete": 51, "backspace": 51,
    "escape": 53, "esc": 53, "forwarddelete": 117, "home": 115, "end": 119, "pageup": 116,
    "pagedown": 121, "left": 123, "right": 124, "down": 125, "up": 126,
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97, "f7": 98, "f8": 100,
    "f9": 101, "f10": 109, "f11": 103, "f12": 111,
  ]

  private static let characters: [String: CGKeyCode] = [
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11,
    "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21,
    "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29, "]": 30, "o": 31,
    "u": 32, "[": 33, "i": 34, "p": 35, "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42,
    ",": 43, "/": 44, "n": 45, "m": 46, ".": 47, "`": 50,
  ]
}
