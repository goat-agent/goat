public struct Outcome: Sendable {
  public enum Result: Sendable, Equatable {
    case done(Delivery)
    case notDone(Reason)
  }

  public enum Delivery: Sendable, Equatable {
    case background, foreground
  }

  public enum Reason: Sendable, Equatable {
    case needsForeground, declined, minimized
  }

  public let result: Result
  public let view: View?

  static func notDone(_ reason: Reason) -> Outcome {
    Outcome(result: .notDone(reason), view: nil)
  }
}
