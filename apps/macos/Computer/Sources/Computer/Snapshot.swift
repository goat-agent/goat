import ApplicationServices
import GoatAPI

/// Reads a window's accessibility tree into the element list the model sees: elements it can act
/// on or read, in tree order, with empty layout containers folded away.
struct Snapshot {
  static let maxNodes = 400
  private static let maxDepth = 40
  private static let maxText = 300

  private(set) var nodes: [Components.Schemas.ComputerNode] = []
  private(set) var refs: Set<String> = []
  private(set) var isTruncated = false

  private static let attributes = [
    kAXRoleAttribute, kAXSubroleAttribute, kAXTitleAttribute, kAXDescriptionAttribute,
    kAXValueAttribute, kAXPlaceholderValueAttribute, kAXEnabledAttribute, kAXFocusedAttribute,
    kAXSelectedAttribute, kAXExpandedAttribute, kAXPositionAttribute, kAXSizeAttribute,
  ]

  /// Layout roles that say nothing on their own; without a label they are folded away.
  private static let layout: Set<String> = [
    "AXGroup", "AXSplitGroup", "AXScrollArea", "AXLayoutArea", "AXLayoutItem", "AXUnknown",
    "AXSplitter", "AXGrowArea", "AXMatte", "AXRuler", "AXColumn", "AXScrollBar", "AXValueIndicator",
  ]

  /// Roles that only hold others; they are listed for structure but get no ref.
  private static let containers: Set<String> = [
    "AXToolbar", "AXList", "AXTable", "AXOutline", "AXTabGroup", "AXWebArea", "AXWindow",
    "AXBrowser", "AXRadioGroup", "AXMenuBar", "AXMenu", "AXSheet", "AXDrawer",
  ]

  init(window: AXUIElement, id: CGWindowID, frame: CGRect, elements: Elements) {
    visit(window, depth: 0, path: [], window: id, frame: frame, elements: elements)
  }

  private mutating func visit(
    _ element: AXUIElement, depth: Int, path: [Int], window: CGWindowID, frame: CGRect,
    elements: Elements
  ) {
    guard nodes.count < Self.maxNodes else {
      isTruncated = true
      return
    }
    let values = element.attributes(Self.attributes)
    let role = values[kAXRoleAttribute] as? String ?? "AXUnknown"
    if let position = values[kAXPositionAttribute], let size = values[kAXSizeAttribute],
      let rect = AXUIElement.rect(position: position, size: size),
      rect.width > 0, rect.height > 0, !rect.intersects(frame)
    {
      return
    }

    let subrole = values[kAXSubroleAttribute] as? String
    let label = Self.label(values)
    let isSecret = subrole == "AXSecureTextField"
    let value = isSecret ? nil : Self.text(values[kAXValueAttribute])
    let isFolded =
      depth > 0 && label == nil
      && (Self.layout.contains(role) || (role == "AXStaticText" && value == nil))

    var childDepth = depth
    if !isFolded && depth > 0 {
      var node = Components.Schemas.ComputerNode(
        depth: depth - 1, role: Self.name(role: role, subrole: subrole))
      node.label = label
      node.value = role == "AXCheckBox" || role == "AXRadioButton" ? nil : value
      let states = Self.states(values, role: role)
      node.states = states.isEmpty ? nil : states
      if !Self.containers.contains(role) {
        let ref = elements.ref(
          for: .init(element: element, window: window, role: role, label: label, path: path))
        node.ref = ref
        refs.insert(ref)
      }
      nodes.append(node)
      childDepth += 1
    } else if depth == 0 {
      childDepth = 1
    }

    guard childDepth <= Self.maxDepth else { return }
    for (index, child) in element.children().enumerated() {
      visit(
        child, depth: childDepth, path: path + [index], window: window, frame: frame,
        elements: elements)
    }
  }

  private static func label(_ values: [String: AnyObject]) -> String? {
    for key in [kAXTitleAttribute, kAXDescriptionAttribute, kAXPlaceholderValueAttribute] {
      if let text = text(values[key]) { return text }
    }
    return nil
  }

  private static func text(_ value: AnyObject?) -> String? {
    let text: String? =
      switch value {
      case let string as String: string
      case let attributed as NSAttributedString: attributed.string
      case let number as NSNumber: number.stringValue
      default: nil
      }
    guard let trimmed = text?.trimmingCharacters(in: .whitespacesAndNewlines), !trimmed.isEmpty
    else { return nil }
    return trimmed.count > maxText ? String(trimmed.prefix(maxText)) + "…" : trimmed
  }

  private static func states(_ values: [String: AnyObject], role: String) -> [String] {
    var states: [String] = []
    if (values[kAXFocusedAttribute] as? NSNumber)?.boolValue == true { states.append("focused") }
    if (values[kAXSelectedAttribute] as? NSNumber)?.boolValue == true { states.append("selected") }
    if (values[kAXExpandedAttribute] as? NSNumber)?.boolValue == true { states.append("expanded") }
    if role == "AXCheckBox" || role == "AXRadioButton",
      (values[kAXValueAttribute] as? NSNumber)?.intValue == 1
    {
      states.append("checked")
    }
    if (values[kAXEnabledAttribute] as? NSNumber)?.boolValue == false { states.append("disabled") }
    return states
  }

  private static let subroleNames = [
    "AXSearchField": "search field",
    "AXSecureTextField": "secure text field",
    "AXCloseButton": "close button",
    "AXMinimizeButton": "minimize button",
    "AXZoomButton": "zoom button",
    "AXFullScreenButton": "full screen button",
    "AXTabButton": "tab",
    "AXToggle": "toggle",
    "AXSwitch": "switch",
  ]

  /// "AXPopUpButton" reads as "pop up button": the model knows these words better than codes.
  static func name(role: String, subrole: String?) -> String {
    if let subrole, let name = subroleNames[subrole] { return name }
    var words: [String] = []
    var word = ""
    for character in role.dropFirst(role.hasPrefix("AX") ? 2 : 0) {
      if character.isUppercase, !word.isEmpty, word.last?.isUppercase == false {
        words.append(word)
        word = ""
      }
      word.append(character)
    }
    if !word.isEmpty { words.append(word) }
    return words.map { $0.lowercased() }.joined(separator: " ")
  }
}
