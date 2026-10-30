import ApplicationServices
import GoatAPI

/// The elements behind the refs a window's element list hands out. A ref stays the same for as
/// long as its element exists, so a ref from an earlier look still works if nothing moved.
final class Elements {
  struct Entry {
    let element: AXUIElement
    let window: CGWindowID
    /// What the element was when the agent saw it, to find it again if the app rebuilt it.
    let role: String
    let label: String?
    let path: [Int]
  }

  private var refs: [AXUIElement: String] = [:]
  private var entries: [String: Entry] = [:]
  private var next = 1

  func ref(for entry: Entry) -> String {
    if let ref = refs[entry.element] {
      entries[ref] = entry
      return ref
    }
    let ref = String(next)
    next += 1
    refs[entry.element] = ref
    entries[ref] = entry
    return ref
  }

  func entry(_ ref: String) -> Entry? {
    entries[ref]
  }

  /// Drops what a window's last two looks did not contain, so memory follows the screen.
  func keep(_ kept: Set<String>, in window: CGWindowID) {
    for (ref, entry) in entries where entry.window == window && !kept.contains(ref) {
      entries[ref] = nil
      refs[entry.element] = nil
    }
  }
}
