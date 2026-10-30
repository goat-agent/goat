import ApplicationServices

/// Sets an element to a value the way its kind expects, all through accessibility.
enum Values {
  static func set(_ element: AXUIElement, role: String, to value: String) throws {
    switch role {
    case "AXCheckBox", "AXRadioButton":
      let wanted = ["1", "true", "on", "yes", "checked"].contains(value.lowercased())
      let current = (element.attribute(kAXValueAttribute) as? NSNumber)?.intValue == 1
      if wanted != current, !element.perform(kAXPressAction) {
        throw ComputerError.cannotSet("The checkbox did not respond.")
      }
    case "AXPopUpButton", "AXMenuButton":
      try choose(value, from: element)
    case "AXSlider", "AXIncrementor", "AXProgressIndicator":
      guard let number = Double(value), element.set(kAXValueAttribute, to: number as NSNumber)
      else { throw ComputerError.cannotSet("Give the slider a number within its range.") }
    default:
      guard element.isSettable(kAXValueAttribute),
        element.set(kAXValueAttribute, to: value as CFString)
      else { throw ComputerError.cannotSet("This element's value cannot be set; try typing.") }
    }
  }

  /// Opens a pop-up menu and picks the item with this title.
  private static func choose(_ title: String, from element: AXUIElement) throws {
    guard element.perform(kAXPressAction) else {
      throw ComputerError.cannotSet("The menu did not open.")
    }
    let items = element.children().flatMap { $0.children() }
    if let item = items.first(where: { Menus.matches($0.string(kAXTitleAttribute), title) }),
      item.perform(kAXPressAction)
    {
      return
    }
    element.children().first?.perform(kAXCancelAction)
    let titles = items.compactMap { $0.string(kAXTitleAttribute) }.filter { !$0.isEmpty }
    throw ComputerError.cannotSet(
      "No item named \(title). Items: \(titles.joined(separator: ", ")).")
  }
}
