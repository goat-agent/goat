import CoreGraphics

/// Where to slide a window so a strip of it shows. Chromium stops drawing a window, and applying
/// its scrolls, while every part of it is covered; a sliver in view keeps it live.
enum Peek {
  static let strip: CGFloat = 40
  private static let step: CGFloat = 16

  static func isFullyCovered(_ frame: CGRect, by covers: [CGRect], screens: [CGRect]) -> Bool {
    visibleSamples(in: frame, covers: covers, screens: screens) == 0
  }

  /// The nearest origin that leaves a strip of the window in view, if any.
  static func origin(for frame: CGRect, covers: [CGRect], screens: [CGRect]) -> CGPoint? {
    let overlapping = covers.filter { $0.intersects(frame) }
    guard let first = overlapping.first else { return nil }
    let union = overlapping.dropFirst().reduce(first) { $0.union($1) }
    let candidates = [
      CGPoint(x: frame.minX, y: union.maxY + strip - frame.height),
      CGPoint(x: frame.minX, y: union.minY - strip),
      CGPoint(x: union.maxX + strip - frame.width, y: frame.minY),
      CGPoint(x: union.minX - strip, y: frame.minY),
    ]
    return
      candidates
      .filter {
        visibleSamples(in: CGRect(origin: $0, size: frame.size), covers: covers, screens: screens)
          >= 4
      }
      .min { distance($0, frame.origin) < distance($1, frame.origin) }
  }

  private static func visibleSamples(in frame: CGRect, covers: [CGRect], screens: [CGRect]) -> Int {
    var count = 0
    var y = frame.minY + step / 2
    while y < frame.maxY {
      var x = frame.minX + step / 2
      while x < frame.maxX {
        let point = CGPoint(x: x, y: y)
        if screens.contains(where: { $0.contains(point) })
          && !covers.contains(where: { $0.contains(point) })
        {
          count += 1
        }
        x += step
      }
      y += step
    }
    return count
  }

  private static func distance(_ a: CGPoint, _ b: CGPoint) -> CGFloat {
    abs(a.x - b.x) + abs(a.y - b.y)
  }
}
