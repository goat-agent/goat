import CoreGraphics

/// A window as the window server lists it. Frames are in global points, origin at the top left.
struct Window: Sendable {
  let id: CGWindowID
  let pid: pid_t
  let app: String
  let title: String
  let frame: CGRect
  let isOnScreen: Bool
}
