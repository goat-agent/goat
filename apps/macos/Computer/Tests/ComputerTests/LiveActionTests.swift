import AppKit
import GoatAPI
import Synchronization
import Testing

@testable import Computer

/// Opens Calculator behind the user's windows and works it through accessibility only, so the
/// pointer and the front app stay put. Runs only when asked:
/// `GOAT_LIVE_ACTIONS=1 swift test --filter LiveActionTests`.
@Suite(.enabled(if: ProcessInfo.processInfo.environment["GOAT_LIVE_ACTIONS"] == "1"))
struct LiveActionTests {
  @Test func usesCalculatorInTheBackground() async throws {
    let wasRunning = !NSRunningApplication.runningApplications(
      withBundleIdentifier: "com.apple.calculator"
    ).isEmpty
    let front = NSWorkspace.shared.frontmostApplication?.processIdentifier
    let pointer = CGEvent(source: nil)?.location
    let borrows = Mutex<[String?]>([])
    let computer = Computer { app in borrows.withLock { $0.append(app) } }

    // Calculator makes its first window only in front, so opening it may borrow the screen.
    let opened = try await computer.open(app: "Calculator", path: nil, url: nil)
    borrows.withLock { $0.removeAll() }
    var view = try #require(opened.view)
    print("opened:", view.window.app, view.nodes.count, "elements")

    for key in ["All Clear", "7", "Multiply", "6", "Equals"] {
      let button = try #require(
        view.nodes.first { $0.role == "button" && $0.label == key }, "no \(key) button")
      let outcome = try await computer.click(
        .init(element: button.ref), button: nil, count: nil, modifiers: [])
      view = try #require(outcome.view)
    }
    let shown = view.nodes.compactMap { $0.value ?? $0.label }
    print("shown:", shown.filter { $0.contains("42") })
    #expect(shown.contains { $0.contains("42") })

    let didBorrow = borrows.withLock { !$0.isEmpty }
    #expect(!didBorrow, "a click borrowed the screen")
    #expect(NSWorkspace.shared.frontmostApplication?.processIdentifier == front)
    #expect(CGEvent(source: nil)?.location == pointer)

    if !wasRunning {
      for app in NSRunningApplication.runningApplications(
        withBundleIdentifier: "com.apple.calculator")
      {
        app.terminate()
      }
    }
  }
}
