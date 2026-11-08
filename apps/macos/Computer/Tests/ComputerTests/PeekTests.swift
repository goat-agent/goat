import CoreGraphics
import Testing

@testable import Computer

struct PeekTests {
  private let screen = CGRect(x: 0, y: 0, width: 1600, height: 1000)

  @Test func aWindowUnderABiggerOneIsCovered() {
    let window = CGRect(x: 200, y: 200, width: 400, height: 300)
    let cover = CGRect(x: 100, y: 100, width: 800, height: 600)
    #expect(Peek.isFullyCovered(window, by: [cover], screens: [screen]))
    #expect(!Peek.isFullyCovered(window, by: [], screens: [screen]))
  }

  @Test func slidesTheShortestWayUntilAStripShows() throws {
    let window = CGRect(x: 200, y: 200, width: 400, height: 300)
    let cover = CGRect(x: 100, y: 100, width: 800, height: 450)
    let origin = try #require(Peek.origin(for: window, covers: [cover], screens: [screen]))
    #expect(origin == CGPoint(x: 200, y: 290))
    let moved = CGRect(origin: origin, size: window.size)
    #expect(!Peek.isFullyCovered(moved, by: [cover], screens: [screen]))
  }

  @Test func findsNoPlaceWhenTheCoverFillsTheScreen() {
    let window = CGRect(x: 200, y: 200, width: 400, height: 300)
    #expect(Peek.origin(for: window, covers: [screen], screens: [screen]) == nil)
  }
}
