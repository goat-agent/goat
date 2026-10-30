import ApplicationServices
import CoreGraphics

/// The window server's id for the window an accessibility element stands for. Private, but the
/// only exact link between the two worlds; window managers have relied on it for years.
@_silgen_name("_AXUIElementGetWindow")
private func axWindowID(_ element: AXUIElement, _ id: UnsafeMutablePointer<CGWindowID>) -> AXError

/// Thin, typed reads and writes over the C accessibility API.
extension AXUIElement {
  static func application(_ pid: pid_t) -> AXUIElement {
    let element = AXUIElementCreateApplication(pid)
    // A hung app would otherwise block every call for the six-second default.
    AXUIElementSetMessagingTimeout(element, 1)
    return element
  }

  func attribute(_ name: String) -> AnyObject? {
    var value: AnyObject?
    guard AXUIElementCopyAttributeValue(self, name as CFString, &value) == .success else {
      return nil
    }
    return value
  }

  /// One round trip for several attributes; missing ones come back as nil.
  func attributes(_ names: [String]) -> [String: AnyObject] {
    var values: CFArray?
    let result = AXUIElementCopyMultipleAttributeValues(
      self, names as CFArray, AXCopyMultipleAttributeOptions(rawValue: 0), &values)
    guard result == .success, let array = values as? [AnyObject] else { return [:] }
    var found: [String: AnyObject] = [:]
    for (name, value) in zip(names, array) where !Self.isError(value) {
      found[name] = value
    }
    return found
  }

  /// A missing attribute comes back from a multiple read as an AXValue holding an error.
  private static func isError(_ value: AnyObject) -> Bool {
    CFGetTypeID(value) == AXValueGetTypeID()
      && AXValueGetType(unsafeDowncast(value, to: AXValue.self)) == .axError
  }

  func element(_ name: String) -> AXUIElement? {
    guard let value = attribute(name), CFGetTypeID(value) == AXUIElementGetTypeID() else {
      return nil
    }
    return unsafeDowncast(value, to: AXUIElement.self)
  }

  func string(_ name: String) -> String? {
    attribute(name) as? String
  }

  func bool(_ name: String) -> Bool? {
    (attribute(name) as? NSNumber)?.boolValue
  }

  func children() -> [AXUIElement] {
    guard let value = attribute(kAXChildrenAttribute) else { return [] }
    return (value as? [AXUIElement]) ?? []
  }

  func actions() -> [String] {
    var names: CFArray?
    guard AXUIElementCopyActionNames(self, &names) == .success else { return [] }
    return (names as? [String]) ?? []
  }

  func isSettable(_ name: String) -> Bool {
    var settable = DarwinBoolean(false)
    return AXUIElementIsAttributeSettable(self, name as CFString, &settable) == .success
      && settable.boolValue
  }

  @discardableResult
  func set(_ name: String, to value: AnyObject) -> Bool {
    AXUIElementSetAttributeValue(self, name as CFString, value) == .success
  }

  @discardableResult
  func perform(_ action: String) -> Bool {
    AXUIElementPerformAction(self, action as CFString) == .success
  }

  var pid: pid_t? {
    var pid: pid_t = 0
    return AXUIElementGetPid(self, &pid) == .success ? pid : nil
  }

  var windowID: CGWindowID? {
    var id: CGWindowID = 0
    return axWindowID(self, &id) == .success && id != 0 ? id : nil
  }

  /// The element's frame in global points, origin at the top left of the main display.
  var frame: CGRect? {
    let values = attributes([kAXPositionAttribute, kAXSizeAttribute])
    guard let position = values[kAXPositionAttribute], let size = values[kAXSizeAttribute] else {
      return nil
    }
    return Self.rect(position: position, size: size)
  }

  /// Whether the element still exists in its app.
  var isAlive: Bool {
    var value: AnyObject?
    return AXUIElementCopyAttributeValue(self, kAXRoleAttribute as CFString, &value)
      != .invalidUIElement
  }

  static func rect(position: AnyObject, size: AnyObject) -> CGRect? {
    guard CFGetTypeID(position) == AXValueGetTypeID(), CFGetTypeID(size) == AXValueGetTypeID()
    else { return nil }
    var point = CGPoint.zero
    var extent = CGSize.zero
    guard AXValueGetValue(unsafeDowncast(position, to: AXValue.self), .cgPoint, &point),
      AXValueGetValue(unsafeDowncast(size, to: AXValue.self), .cgSize, &extent)
    else { return nil }
    return CGRect(origin: point, size: extent)
  }
}
