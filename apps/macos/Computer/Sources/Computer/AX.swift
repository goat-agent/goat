import ApplicationServices

extension AXUIElement {
  static func application(_ pid: pid_t) -> AXUIElement {
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 2)
    return app
  }

  func value(_ attribute: String) -> CFTypeRef? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(self, attribute as CFString, &value) == .success else {
      return nil
    }
    return value
  }

  func string(_ attribute: String) -> String? {
    value(attribute) as? String
  }

  func bool(_ attribute: String) -> Bool {
    (value(attribute) as? Bool) ?? false
  }

  func element(_ attribute: String) -> AXUIElement? {
    guard let value = value(attribute), CFGetTypeID(value) == AXUIElementGetTypeID() else {
      return nil
    }
    return unsafeDowncast(value, to: AXUIElement.self)
  }

  func elements(_ attribute: String) -> [AXUIElement] {
    (value(attribute) as? [AXUIElement]) ?? []
  }

  var role: String? { string(kAXRoleAttribute) }

  var frame: CGRect? {
    guard let position = axValue(kAXPositionAttribute), let size = axValue(kAXSizeAttribute)
    else { return nil }
    var origin = CGPoint.zero
    var extent = CGSize.zero
    guard AXValueGetValue(position, .cgPoint, &origin), AXValueGetValue(size, .cgSize, &extent)
    else { return nil }
    return CGRect(origin: origin, size: extent)
  }

  var actions: [String] {
    var names: CFArray?
    guard AXUIElementCopyActionNames(self, &names) == .success else { return [] }
    return (names as? [String]) ?? []
  }

  @discardableResult
  func set(_ attribute: String, to value: CFTypeRef) -> Bool {
    AXUIElementSetAttributeValue(self, attribute as CFString, value) == .success
  }

  @discardableResult
  func move(to origin: CGPoint) -> Bool {
    var origin = origin
    guard let value = AXValueCreate(.cgPoint, &origin) else { return false }
    return set(kAXPositionAttribute, to: value)
  }

  @discardableResult
  func perform(_ action: String) -> Bool {
    AXUIElementPerformAction(self, action as CFString) == .success
  }

  /// The deepest element of this app under a screen point.
  func hit(_ point: CGPoint) -> AXUIElement? {
    var element: AXUIElement?
    guard
      AXUIElementCopyElementAtPosition(self, Float(point.x), Float(point.y), &element) == .success
    else { return nil }
    return element
  }

  private func axValue(_ attribute: String) -> AXValue? {
    guard let value = value(attribute), CFGetTypeID(value) == AXValueGetTypeID() else {
      return nil
    }
    return unsafeDowncast(value, to: AXValue.self)
  }
}
