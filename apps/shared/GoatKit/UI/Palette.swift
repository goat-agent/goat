import SwiftUI

/// Every color the screens draw with, named as in apps/web/src/styles.css so both say the same
/// thing.
enum Palette {
  /// The window behind the sidebar and the content.
  static let sidebar = Color(light: Color(hex: 0xECECEC), dark: Color(hex: 0x0E0E0E))
  /// The conversation card and other content surfaces.
  static let background = Color(light: .white, dark: Color(hex: 0x1C1C1C))
  /// Menus, dialogs, and panels that float over the content.
  static let popover = Color(light: .white, dark: Color(hex: 0x323232))
  static let foreground = Color(light: Color(hex: 0x1D1D1F), dark: Color(hex: 0xF5F5F7))
  /// Secondary text, dark enough to keep 4.5:1 against the sidebar and a selected row.
  static let mutedForeground = Color(light: Color(hex: 0x5F5F64), dark: Color(hex: 0x98989D))
  /// The one next action on a screen, and the user's own messages.
  static let primary = Color(hex: 0x7C3AED)
  static let primaryForeground = Color.white
  static let secondary = Color(light: .black.opacity(0.05), dark: .white.opacity(0.07))
  static let muted = Color(light: .black.opacity(0.08), dark: .white.opacity(0.11))
  /// Under the pointer.
  static let accent = Color(light: .black.opacity(0.03), dark: .white.opacity(0.04))
  /// The selected row in a list.
  static let sidebarAccent = Color(light: .black.opacity(0.06), dark: .white.opacity(0.08))
  static let input = Color(light: .black.opacity(0.05), dark: .white.opacity(0.07))
  /// The outline of the focused control.
  static let ring = Color(
    light: Color(hex: 0x1D1D1F, opacity: 0.28), dark: Color(hex: 0xF5F5F7, opacity: 0.28))
  static let border = Color(light: .black.opacity(0.08), dark: .white.opacity(0.09))
  static let destructive = Color(light: Color(hex: 0xE5342B), dark: Color(hex: 0xFF5B52))
}

extension Color {
  fileprivate init(hex: UInt32, opacity: Double = 1) {
    self.init(
      red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255,
      blue: Double(hex & 0xFF) / 255, opacity: opacity)
  }

  init(light: Color, dark: Color) {
    #if os(macOS)
      self.init(
        nsColor: NSColor(name: nil) { appearance in
          NSColor(appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light)
        })
    #else
      self.init(
        uiColor: UIColor { traits in
          UIColor(traits.userInterfaceStyle == .dark ? dark : light)
        })
    #endif
  }
}
