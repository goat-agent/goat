import ApplicationServices

/// Presses controls through accessibility, which reaches Catalyst apps that ignore posted pointer
/// events.
enum AccessibilityPress {
  private static let controls: Set<String> = [
    kAXButtonRole, kAXRadioButtonRole, kAXCheckBoxRole, kAXMenuButtonRole, kAXPopUpButtonRole,
    kAXDisclosureTriangleRole,
  ]

  /// Presses the control under a screen point. Web pages are left to events, which they take
  /// more reliably than a press.
  static func pressControl(at point: CGPoint, pid: pid_t) -> Bool {
    guard let hit = AXUIElement.application(pid).hit(point) else { return false }
    var candidate: AXUIElement? = hit
    for _ in 0..<4 {
      guard let element = candidate else { return false }
      if let role = element.role, controls.contains(role), element.actions.contains(kAXPressAction)
      {
        return !isInWebPage(element) && element.perform(kAXPressAction)
      }
      candidate = element.element(kAXParentAttribute)
    }
    return false
  }

  private static func isInWebPage(_ element: AXUIElement) -> Bool {
    var current: AXUIElement? = element
    for _ in 0..<40 {
      guard let next = current else { return false }
      if next.role == "AXWebArea" { return true }
      current = next.element(kAXParentAttribute)
    }
    return false
  }
}
