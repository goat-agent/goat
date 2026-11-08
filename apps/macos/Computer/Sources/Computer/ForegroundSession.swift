import AppKit
import ApplicationServices

/// Brings an app to the front for one action, then gives the user back their pointer and app.
struct ForegroundSession {
  let pid: pid_t
  let window: AXUIElement?

  func run<T: Sendable>(_ body: @Sendable () async throws -> T) async throws -> T {
    let previous = await MainActor.run {
      NSWorkspace.shared.frontmostApplication?.processIdentifier
    }
    let pointer = CGEvent(source: nil)?.location
    if let window, window.bool(kAXMinimizedAttribute) {
      window.set(kAXMinimizedAttribute, to: kCFBooleanFalse)
      try await Task.sleep(for: .milliseconds(500))
    }
    let pid = pid
    await MainActor.run { _ = NSRunningApplication(processIdentifier: pid)?.activate() }
    window?.set(kAXMainAttribute, to: kCFBooleanTrue)
    window?.perform(kAXRaiseAction)
    try await Task.sleep(for: .milliseconds(400))
    do {
      let result = try await body()
      await restore(previous, pointer: pointer)
      return result
    } catch {
      await restore(previous, pointer: pointer)
      throw error
    }
  }

  private func restore(_ previous: pid_t?, pointer: CGPoint?) async {
    if let pointer { CGWarpMouseCursorPosition(pointer) }
    guard let previous, previous != pid else { return }
    await MainActor.run { _ = NSRunningApplication(processIdentifier: previous)?.activate() }
  }
}
