import Foundation
import Testing

@testable import Computer

/// Reads the real screen; nothing is clicked or typed. Needs Accessibility and Screen Recording
/// for the process running the tests, so it runs only when asked:
/// `GOAT_LIVE_COMPUTER=1 swift test --filter LiveTests`.
@Suite(.enabled(if: ProcessInfo.processInfo.environment["GOAT_LIVE_COMPUTER"] == "1"))
struct LiveTests {
  @Test func looksAtTheScreenAndTheFrontWindow() async throws {
    let computer = Computer { _ in }
    let started = ContinuousClock.now
    let screen = try await computer.screen(includeImage: true)
    let image = try #require(screen.image)
    #expect(max(image.width, image.height) <= 1280)
    #expect(Data(base64Encoded: image.data) != nil)
    print("screen:", screen.windows.count, "windows,", ContinuousClock.now - started)

    for window in screen.windows where !window.isMinimized {
      let looked = ContinuousClock.now
      let view = try await computer.view(window: window.ref)
      #expect(max(view.image.width, view.image.height) <= 1280)
      print(
        "view:", window.app, "—", view.nodes.count, "elements,", view.image.width, "×",
        view.image.height, ContinuousClock.now - looked)
      for node in view.nodes.prefix(12) {
        let label = node.label.map { " \"\($0.prefix(40))\"" } ?? ""
        let ref = node.ref.map { " [\($0)]" } ?? ""
        print(String(repeating: "  ", count: node.depth + 1) + "- " + node.role + label + ref)
      }
    }
  }
}
