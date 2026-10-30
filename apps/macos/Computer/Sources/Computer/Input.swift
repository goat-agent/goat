import Carbon.HIToolbox
import CoreGraphics
import Foundation

/// Synthetic keyboard and pointer events. Keys can go to one app in the background; the pointer
/// is the user's, so pointer events only run while the screen is borrowed.
enum Input {
  enum Destination {
    /// Straight to one app, leaving the user's front app and focus alone.
    case app(pid_t)
    /// Through the system, as if from the hardware; only while the screen is borrowed.
    case system
  }

  /// A private state, so events posted here never inherit keys the user is holding.
  private static var source: CGEventSource? { CGEventSource(stateID: .privateState) }

  // MARK: Keys

  struct Chord {
    let key: CGKeyCode
    let flags: CGEventFlags
  }

  /// Parses "cmd+shift+t", "return", or "f5".
  static func chord(_ keys: String) throws -> Chord {
    var flags = CGEventFlags()
    var key: CGKeyCode?
    for part in keys.lowercased().split(separator: "+").map(String.init) {
      if let flag = modifier(part) {
        flags.insert(flag)
      } else if key == nil, let code = keyCodes[part] {
        key = code
      } else {
        throw ComputerError.badKeys(keys)
      }
    }
    guard let key else { throw ComputerError.badKeys(keys) }
    return Chord(key: key, flags: flags)
  }

  static func flags(_ modifiers: [String]) -> CGEventFlags {
    modifiers.reduce(into: CGEventFlags()) { flags, name in
      if let flag = modifier(name.lowercased()) { flags.insert(flag) }
    }
  }

  static func press(_ chord: Chord, times: Int, to destination: Destination) async {
    for _ in 0..<max(1, times) {
      for isDown in [true, false] {
        guard
          let event = CGEvent(keyboardEventSource: source, virtualKey: chord.key, keyDown: isDown)
        else { continue }
        event.flags = chord.flags
        post(event, to: destination)
      }
      try? await Task.sleep(for: .milliseconds(15))
    }
  }

  /// Types text as characters, so it does not depend on the keyboard layout or input method.
  static func type(_ text: String, to destination: Destination) async {
    let units = Array(text.utf16)
    var start = 0
    while start < units.count {
      // Events carry at most 20 UTF-16 units; never split a surrogate pair.
      var end = min(start + 20, units.count)
      if end < units.count, UTF16.isLeadSurrogate(units[end - 1]) { end -= 1 }
      let chunk = Array(units[start..<end])
      for isDown in [true, false] {
        guard let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: isDown)
        else { continue }
        event.keyboardSetUnicodeString(stringLength: chunk.count, unicodeString: chunk)
        post(event, to: destination)
      }
      start = end
      try? await Task.sleep(for: .milliseconds(8))
    }
  }

  // MARK: Pointer

  static func move(to point: CGPoint) {
    CGWarpMouseCursorPosition(point)
    mouse(.mouseMoved, at: point, button: .left, flags: [])
  }

  static func click(
    at point: CGPoint, button: CGMouseButton, count: Int, flags: CGEventFlags
  ) async {
    move(to: point)
    try? await Task.sleep(for: .milliseconds(30))
    let (down, up) = types(for: button)
    for click in 1...max(1, count) {
      mouse(down, at: point, button: button, flags: flags, clickState: click)
      mouse(up, at: point, button: button, flags: flags, clickState: click)
      try? await Task.sleep(for: .milliseconds(40))
    }
  }

  static func drag(from start: CGPoint, to end: CGPoint, flags: CGEventFlags) async {
    move(to: start)
    try? await Task.sleep(for: .milliseconds(40))
    mouse(.leftMouseDown, at: start, button: .left, flags: flags)
    let steps = 20
    for step in 1...steps {
      let progress = CGFloat(step) / CGFloat(steps)
      let point = CGPoint(
        x: start.x + (end.x - start.x) * progress, y: start.y + (end.y - start.y) * progress)
      mouse(.leftMouseDragged, at: point, button: .left, flags: flags)
      try? await Task.sleep(for: .milliseconds(15))
    }
    mouse(.leftMouseUp, at: end, button: .left, flags: flags)
  }

  static func scroll(at point: CGPoint, dx: Int32, dy: Int32) async {
    move(to: point)
    try? await Task.sleep(for: .milliseconds(30))
    for _ in 0..<max(abs(dx), abs(dy)) {
      guard
        let event = CGEvent(
          scrollWheelEvent2Source: source, units: .line, wheelCount: 2,
          wheel1: dy.signum(), wheel2: dx.signum(), wheel3: 0)
      else { continue }
      event.location = point
      event.post(tap: .cghidEventTap)
      try? await Task.sleep(for: .milliseconds(20))
    }
  }

  // MARK: Plumbing

  private static func mouse(
    _ type: CGEventType, at point: CGPoint, button: CGMouseButton, flags: CGEventFlags,
    clickState: Int = 1
  ) {
    guard
      let event = CGEvent(
        mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: button)
    else { return }
    event.flags = flags
    event.setIntegerValueField(.mouseEventClickState, value: Int64(clickState))
    event.post(tap: .cghidEventTap)
  }

  private static func types(for button: CGMouseButton) -> (CGEventType, CGEventType) {
    switch button {
    case .right: (.rightMouseDown, .rightMouseUp)
    case .center: (.otherMouseDown, .otherMouseUp)
    default: (.leftMouseDown, .leftMouseUp)
    }
  }

  private static func post(_ event: CGEvent, to destination: Destination) {
    switch destination {
    case .app(let pid): event.postToPid(pid)
    case .system: event.post(tap: .cghidEventTap)
    }
  }

  private static func modifier(_ name: String) -> CGEventFlags? {
    switch name {
    case "cmd", "command": .maskCommand
    case "shift": .maskShift
    case "option", "opt", "alt": .maskAlternate
    case "ctrl", "control": .maskControl
    case "fn": .maskSecondaryFn
    default: nil
    }
  }

  private static let keyCodes: [String: CGKeyCode] = {
    var codes: [String: Int] = [
      "return": kVK_Return, "enter": kVK_Return, "tab": kVK_Tab, "space": kVK_Space,
      "delete": kVK_Delete, "backspace": kVK_Delete, "forwarddelete": kVK_ForwardDelete,
      "escape": kVK_Escape, "esc": kVK_Escape, "left": kVK_LeftArrow, "right": kVK_RightArrow,
      "up": kVK_UpArrow, "down": kVK_DownArrow, "home": kVK_Home, "end": kVK_End,
      "pageup": kVK_PageUp, "pagedown": kVK_PageDown,
      "a": kVK_ANSI_A, "b": kVK_ANSI_B, "c": kVK_ANSI_C, "d": kVK_ANSI_D, "e": kVK_ANSI_E,
      "f": kVK_ANSI_F, "g": kVK_ANSI_G, "h": kVK_ANSI_H, "i": kVK_ANSI_I, "j": kVK_ANSI_J,
      "k": kVK_ANSI_K, "l": kVK_ANSI_L, "m": kVK_ANSI_M, "n": kVK_ANSI_N, "o": kVK_ANSI_O,
      "p": kVK_ANSI_P, "q": kVK_ANSI_Q, "r": kVK_ANSI_R, "s": kVK_ANSI_S, "t": kVK_ANSI_T,
      "u": kVK_ANSI_U, "v": kVK_ANSI_V, "w": kVK_ANSI_W, "x": kVK_ANSI_X, "y": kVK_ANSI_Y,
      "z": kVK_ANSI_Z, "0": kVK_ANSI_0, "1": kVK_ANSI_1, "2": kVK_ANSI_2, "3": kVK_ANSI_3,
      "4": kVK_ANSI_4, "5": kVK_ANSI_5, "6": kVK_ANSI_6, "7": kVK_ANSI_7, "8": kVK_ANSI_8,
      "9": kVK_ANSI_9, "-": kVK_ANSI_Minus, "=": kVK_ANSI_Equal, "[": kVK_ANSI_LeftBracket,
      "]": kVK_ANSI_RightBracket, ";": kVK_ANSI_Semicolon, "'": kVK_ANSI_Quote,
      ",": kVK_ANSI_Comma, ".": kVK_ANSI_Period, "/": kVK_ANSI_Slash,
      "\\": kVK_ANSI_Backslash, "`": kVK_ANSI_Grave,
    ]
    let functionKeys = [
      kVK_F1, kVK_F2, kVK_F3, kVK_F4, kVK_F5, kVK_F6, kVK_F7, kVK_F8, kVK_F9, kVK_F10, kVK_F11,
      kVK_F12,
    ]
    for (index, code) in functionKeys.enumerated() { codes["f\(index + 1)"] = code }
    return codes.mapValues { CGKeyCode($0) }
  }()
}
