import SwiftUI

/// An icon's place in a row. The place, not the symbol, sets the size: every symbol is fitted
/// into the same box, so a wide one and a narrow one look alike and the text after them starts
/// on one line.
struct IconSlot: View {
  enum Size {
    /// Beside a label: page lists, menu items, a quiet line.
    case inline
    /// At the front of a list row, where service logos also sit.
    case item

    #if os(macOS)
      var side: CGFloat { self == .inline ? 20 : 28 }
      fileprivate var glyph: CGFloat { self == .inline ? 15 : 18 }
    #else
      var side: CGFloat { self == .inline ? 24 : 32 }
      fileprivate var glyph: CGFloat { self == .inline ? 18 : 22 }
    #endif
  }

  let systemName: String
  var size = Size.inline

  var body: some View {
    Image(systemName: systemName)
      .resizable()
      .scaledToFit()
      .frame(width: size.glyph, height: size.glyph)
      .frame(width: size.side, height: size.side)
      .accessibilityHidden(true)
  }
}

#Preview {
  let names = ["person.crop.circle", "puzzlepiece.extension", "iphone", "desktopcomputer"]
  VStack(alignment: .leading, spacing: 8) {
    ForEach(names, id: \.self) { name in
      HStack(spacing: 8) {
        IconSlot(systemName: name)
        Text(verbatim: name)
      }
    }
  }
  .padding(24)
}
