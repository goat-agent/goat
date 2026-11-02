import SwiftUI

/// Control sizes, as web's `sm`, `default`, and `lg`: a pointer on the Mac, a finger on the
/// iPhone. Every control reads its height and type from here, so screens never branch by
/// platform.
extension ControlSize {
  var height: CGFloat {
    #if os(macOS)
      switch self {
      case .mini, .small: 26
      case .large, .extraLarge: 40
      default: 32
      }
    #else
      switch self {
      case .mini, .small: 34
      case .large, .extraLarge: 52
      default: 44
      }
    #endif
  }

  var font: Font {
    #if os(macOS)
      switch self {
      case .mini, .small: .system(size: 12, weight: .semibold)
      case .large, .extraLarge: .system(size: 14, weight: .semibold)
      default: .system(size: 13, weight: .semibold)
      }
    #else
      switch self {
      case .mini, .small: .subheadline.weight(.semibold)
      default: .body.weight(.semibold)
      }
    #endif
  }

  /// Text inside an input, which is not bold.
  var inputFont: Font {
    #if os(macOS)
      self == .large || self == .extraLarge ? .system(size: 14) : .system(size: 13)
    #else
      .body
    #endif
  }
}

extension Font {
  /// The heading of a settings page or section.
  static var sectionTitle: Font {
    #if os(macOS)
      .system(size: 20, weight: .semibold)
    #else
      .title3.weight(.semibold)
    #endif
  }
}
