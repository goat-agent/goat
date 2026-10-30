import ApplicationServices

/// Chooses menu bar items through accessibility, which works while the app is in the background.
enum Menus {
  static func choose(_ path: [String], in pid: pid_t) throws {
    guard var level = AXUIElement.application(pid).element(kAXMenuBarAttribute) else {
      throw ComputerError.menuNotFound(path.first ?? "")
    }
    for (index, title) in path.enumerated() {
      let items = level.children()
      guard let item = items.first(where: { matches($0.string(kAXTitleAttribute), title) }) else {
        let titles = items.compactMap { $0.string(kAXTitleAttribute) }.filter { !$0.isEmpty }
        throw ComputerError.menuNotFound(
          "\(title). Items here: \(titles.joined(separator: ", "))")
      }
      if index == path.count - 1 {
        guard item.perform(kAXPressAction) else { throw ComputerError.menuNotFound(title) }
        return
      }
      guard let submenu = item.children().first else { throw ComputerError.menuNotFound(title) }
      level = submenu
    }
  }

  /// Titles match without regard to case or a trailing ellipsis, which apps write two ways.
  static func matches(_ title: String?, _ wanted: String) -> Bool {
    guard let title else { return false }
    return normalized(title) == normalized(wanted)
  }

  private static func normalized(_ title: String) -> String {
    title.replacingOccurrences(of: "…", with: "").replacingOccurrences(of: "...", with: "")
      .trimmingCharacters(in: .whitespaces).lowercased()
  }
}
