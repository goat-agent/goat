import CoreGraphics

/// Synthesized pointer and keyboard input, either posted to one app's process or put on the
/// system queue like the user's own input.
struct Input {
  enum Route {
    case background(pid: pid_t, window: CGWindowID)
    case foreground
  }

  // Not in Apple's headers. Without the window number an app in the background drops posted
  // pointer events; the click group keeps the events of one gesture together.
  private static let windowNumberField = CGEventField(rawValue: 51)
  private static let clickGroupField = CGEventField(rawValue: 58)
  private static let pixelsPerNotch = 40

  let route: Route
  private let source = CGEventSource(stateID: .hidSystemState)
  private let gesture = Int64.random(in: 1...Int64(Int32.max))

  func perform(_ action: Action) async {
    switch action {
    case .click(let point, let button, let count, let modifiers):
      await click(at: point, button: button, count: count, modifiers: modifiers)
    case .drag(let from, let to, let modifiers):
      await drag(from: from, to: to, modifiers: modifiers)
    case .scroll(let point, let direction, let amount):
      await scroll(at: point, direction: direction, amount: amount)
    case .move(let point):
      post(mouse(.mouseMoved, at: point, button: .left, clickState: 0, modifiers: []))
    case .type(let text):
      await type(text)
    case .key(let combo, let count):
      for _ in 0..<max(1, count) { await press(combo) }
    }
  }

  private func click(at point: CGPoint, button: MouseButton, count: Int, modifiers: Modifiers)
    async
  {
    post(mouse(.mouseMoved, at: point, button: .left, clickState: 0, modifiers: []))
    await pause(15)
    for clickState in 1...max(1, count) {
      post(
        mouse(button.down, at: point, button: button, clickState: clickState, modifiers: modifiers))
      await pause(20)
      post(
        mouse(button.up, at: point, button: button, clickState: clickState, modifiers: modifiers))
      await pause(60)
    }
  }

  private func drag(from start: CGPoint, to end: CGPoint, modifiers: Modifiers) async {
    post(mouse(.mouseMoved, at: start, button: .left, clickState: 0, modifiers: []))
    await pause(15)
    post(mouse(.leftMouseDown, at: start, button: .left, clickState: 1, modifiers: modifiers))
    await pause(30)
    let steps = 12
    for step in 1...steps {
      let t = CGFloat(step) / CGFloat(steps)
      let point = CGPoint(x: start.x + (end.x - start.x) * t, y: start.y + (end.y - start.y) * t)
      post(mouse(.leftMouseDragged, at: point, button: .left, clickState: 1, modifiers: modifiers))
      await pause(15)
    }
    post(mouse(.leftMouseUp, at: end, button: .left, clickState: 1, modifiers: modifiers))
  }

  private func scroll(at point: CGPoint, direction: ScrollDirection, amount: Int) async {
    post(mouse(.mouseMoved, at: point, button: .left, clickState: 0, modifiers: []))
    await pause(15)
    let pixels = Int32(amount * Self.pixelsPerNotch)
    let (vertical, horizontal): (Int32, Int32) =
      switch direction {
      case .up: (pixels, 0)
      case .down: (-pixels, 0)
      case .left: (0, pixels)
      case .right: (0, -pixels)
      }
    guard
      let event = CGEvent(
        scrollWheelEvent2Source: source, units: .pixel, wheelCount: 2, wheel1: vertical,
        wheel2: horizontal, wheel3: 0)
    else { return }
    event.location = point
    post(event)
  }

  private func type(_ text: String) async {
    for character in text {
      switch character {
      case "\n", "\r\n": await press(.returnKey)
      case "\t": await press(.tabKey)
      default:
        let units = Array(String(character).utf16)
        for isDown in [true, false] {
          guard let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: isDown)
          else { continue }
          event.flags = []
          event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
          post(event)
          await pause(12)
        }
      }
    }
  }

  private func press(_ combo: KeyCombo) async {
    for isDown in [true, false] {
      guard
        let event = CGEvent(keyboardEventSource: source, virtualKey: combo.code, keyDown: isDown)
      else { continue }
      event.flags = combo.modifiers.flags
      post(event)
      await pause(12)
    }
  }

  private func mouse(
    _ type: CGEventType, at point: CGPoint, button: MouseButton, clickState: Int,
    modifiers: Modifiers
  ) -> CGEvent? {
    guard
      let event = CGEvent(
        mouseEventSource: source, mouseType: type, mouseCursorPosition: point,
        mouseButton: button.cgButton)
    else { return nil }
    event.flags = modifiers.flags
    event.setIntegerValueField(.mouseEventClickState, value: Int64(clickState))
    event.setIntegerValueField(.mouseEventButtonNumber, value: Int64(button.cgButton.rawValue))
    return event
  }

  private func post(_ event: CGEvent?) {
    guard let event else { return }
    switch route {
    case .background(let pid, let window):
      if let field = Self.windowNumberField {
        event.setIntegerValueField(field, value: Int64(window))
      }
      if let field = Self.clickGroupField { event.setIntegerValueField(field, value: gesture) }
      event.postToPid(pid)
    case .foreground:
      event.post(tap: .cghidEventTap)
    }
  }

  private func pause(_ milliseconds: Int) async {
    try? await Task.sleep(for: .milliseconds(milliseconds))
  }
}

extension MouseButton {
  fileprivate var cgButton: CGMouseButton {
    switch self {
    case .left: .left
    case .right: .right
    case .middle: .center
    }
  }

  fileprivate var down: CGEventType {
    switch self {
    case .left: .leftMouseDown
    case .right: .rightMouseDown
    case .middle: .otherMouseDown
    }
  }

  fileprivate var up: CGEventType {
    switch self {
    case .left: .leftMouseUp
    case .right: .rightMouseUp
    case .middle: .otherMouseUp
    }
  }
}
