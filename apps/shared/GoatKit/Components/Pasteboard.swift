#if os(iOS)
  import UIKit
#else
  import AppKit
#endif

/// Copying text, which iPhone and Mac do through different classes.
enum Pasteboard {
  static func copy(_ text: String) {
    #if os(iOS)
      UIPasteboard.general.string = text
    #else
      NSPasteboard.general.clearContents()
      NSPasteboard.general.setString(text, forType: .string)
    #endif
  }
}
