import SwiftUI

/// Search and new agent drawn on one 16 pt grid with one stroke, so they read as the same size.
struct Glyph: View {
  nonisolated enum Kind {
    case search
    case plus
  }

  let kind: Kind
  var size: CGFloat = 16

  var body: some View {
    GlyphShape(kind: kind)
      .stroke(style: StrokeStyle(lineWidth: 1.5 * size / 16, lineCap: .round))
      .frame(width: size, height: size)
  }
}

nonisolated private struct GlyphShape: Shape {
  let kind: Glyph.Kind

  func path(in rect: CGRect) -> Path {
    let unit = rect.width / 16
    func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
      CGPoint(x: rect.minX + x * unit, y: rect.minY + y * unit)
    }
    var path = Path()
    switch kind {
    case .search:
      path.addEllipse(
        in: CGRect(origin: point(2.25, 2.25), size: CGSize(width: 9.5 * unit, height: 9.5 * unit)))
      path.move(to: point(10.5, 10.5))
      path.addLine(to: point(13.75, 13.75))
    case .plus:
      path.move(to: point(8, 2.75))
      path.addLine(to: point(8, 13.25))
      path.move(to: point(2.75, 8))
      path.addLine(to: point(13.25, 8))
    }
    return path
  }
}
