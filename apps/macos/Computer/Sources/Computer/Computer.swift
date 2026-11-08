import AppKit
import ApplicationServices

/// Looks at and acts in the windows of this Mac without taking over the user's screen. Each action
/// runs in the first way that reaches the app from the background: a press through accessibility,
/// then posted events. What only works in front is done in front, and only after the user says yes.
public actor Computer {
  /// Asks the user whether an app may come to the front; `app` is its name.
  public typealias Consent = @Sendable (_ app: String) async -> Bool

  private struct Peeked {
    let original: CGPoint
    let moved: CGPoint
    var until: ContinuousClock.Instant
  }

  private static let settle = Duration.milliseconds(350)
  private static let peekLasts = Duration.seconds(60)

  private let askToBringForward: Consent
  private var imageSizes: [CGWindowID: CGSize] = [:]
  private var kinds: [pid_t: AppKind] = [:]
  private var peeked: [CGWindowID: Peeked] = [:]
  private var restoringPeeks: Task<Void, Never>?

  public init(askToBringForward: @escaping Consent) {
    self.askToBringForward = askToBringForward
  }

  // MARK: Looking

  public func windows() async -> [WindowSummary] {
    let front = await MainActor.run { NSWorkspace.shared.frontmostApplication?.processIdentifier }
    let windows = WindowCatalog.all()
    let frontWindow = windows.first { $0.isOnScreen && $0.pid == front }?.id
    return windows.map { summary(of: $0, isFront: $0.id == frontWindow) }
  }

  public func view(window ref: String, region: CGRect? = nil, wait: Duration? = nil) async throws
    -> View
  {
    if let wait { try await Task.sleep(for: min(wait, .seconds(60))) }
    let window = try resolve(ref)
    if WindowCatalog.isMinimized(window) { throw ComputerError.minimized }
    await keepVisible(window)
    return try await capture(window, region: region)
  }

  // MARK: Acting

  public func perform(_ action: Action, window ref: String, foreground: Bool) async throws
    -> Outcome
  {
    let window = try resolve(ref)
    let element = WindowCatalog.element(of: window)
    if foreground { return try await performInFront(action, window: window, element: element) }
    if element?.bool(kAXMinimizedAttribute) == true { return .notDone(.minimized) }
    if case .move = action { return .notDone(.needsForeground) }

    let kind = kind(of: window.pid)
    await keepVisible(window)
    let current = WindowCatalog.window(window.id) ?? window
    let onScreen = try action.mapped { try screenPoint($0, in: current) }
    element?.set(kAXMainAttribute, to: kCFBooleanTrue)

    if case .click(let point, .left, 1, []) = onScreen,
      AccessibilityPress.pressControl(at: point, pid: window.pid)
    {
      return try await done(.background, window: window)
    }
    if case .key(let combo, _) = action, combo.modifiers.contains(.command), kind == .chromium {
      return .notDone(.needsForeground)
    }
    if kind == .uikit && action.isPointer { return .notDone(.needsForeground) }

    await Input(route: .background(pid: window.pid, window: window.id)).perform(onScreen)
    return try await done(.background, window: window)
  }

  public func open(_ target: OpenTarget, foreground: Bool) async throws -> Outcome {
    if foreground, await !askToBringForward(target.name) { return .notDone(.declined) }
    let pid = try await Self.open(target.url(), activates: foreground)
    for _ in 0..<30 {
      if let window = WindowCatalog.all().first(where: { $0.pid == pid && $0.isOnScreen }) {
        try await Task.sleep(for: Self.settle)
        return Outcome(
          result: .done(foreground ? .foreground : .background),
          view: try await capture(window, region: nil))
      }
      try await Task.sleep(for: .milliseconds(200))
    }
    if !foreground { return .notDone(.needsForeground) }
    throw ComputerError.noWindowOpened
  }

  // MARK: Steps

  private func performInFront(_ action: Action, window: Window, element: AXUIElement?) async throws
    -> Outcome
  {
    guard await askToBringForward(window.app) else { return .notDone(.declined) }
    let id = window.id
    let hovering: Bool
    if case .move = action { hovering = true } else { hovering = false }
    let size = imageSizes[id]
    let view: View? = try await ForegroundSession(pid: window.pid, window: element).run {
      guard let current = WindowCatalog.window(id) else { throw ComputerError.windowGone }
      let onScreen = try action.mapped { try Self.screenPoint($0, in: current, imageSize: size) }
      await Input(route: .foreground).perform(onScreen)
      guard hovering else { return nil }
      try await Task.sleep(for: .milliseconds(700))
      return try await self.capture(current, region: nil)
    }
    if let view { return Outcome(result: .done(.foreground), view: view) }
    return try await done(.foreground, window: window)
  }

  private func done(_ delivery: Outcome.Delivery, window: Window) async throws -> Outcome {
    try await Task.sleep(for: Self.settle)
    let current = WindowCatalog.window(window.id) ?? window
    return Outcome(result: .done(delivery), view: try await capture(current, region: nil))
  }

  private func capture(_ window: Window, region: CGRect?) async throws -> View {
    let full = try await Capture.window(window.id)
    let summary = summary(of: window, isFront: false)
    if let region {
      let shown = imageSizes[window.id] ?? window.frame.size
      let scale = CGFloat(full.width) / shown.width
      let crop = CGRect(
        x: region.minX * scale, y: region.minY * scale, width: region.width * scale,
        height: region.height * scale
      ).integral.intersection(CGRect(x: 0, y: 0, width: full.width, height: full.height))
      guard let zoom = full.cropping(to: crop) else { throw ComputerError.outsideWindow }
      return try encode(zoom, points: crop.size, window: summary)
    }
    let size = Capture.sizeForModel(of: full, points: window.frame.size)
    imageSizes[window.id] = size
    guard let image = Capture.resized(full, to: size) else { throw ComputerError.captureFailed }
    return try encode(image, points: size, window: summary)
  }

  private func encode(_ image: CGImage, points: CGSize, window: WindowSummary) throws -> View {
    let size = Capture.sizeForModel(of: image, points: points)
    guard let scaled = Capture.resized(image, to: size), let jpeg = Capture.jpeg(scaled) else {
      throw ComputerError.captureFailed
    }
    return View(window: window, jpeg: jpeg, width: scaled.width, height: scaled.height)
  }

  /// Slides a fully covered Chromium window so a strip of it shows, and puts it back once the
  /// agent has left it alone for a while.
  private func keepVisible(_ window: Window) async {
    guard kind(of: window.pid) == .chromium else { return }
    if var known = peeked[window.id] {
      known.until = .now + Self.peekLasts
      peeked[window.id] = known
      return
    }
    let covers = WindowCatalog.framesAbove(window)
    let screens = Self.screens()
    guard Peek.isFullyCovered(window.frame, by: covers, screens: screens),
      let origin = Peek.origin(for: window.frame, covers: covers, screens: screens),
      WindowCatalog.element(of: window)?.move(to: origin) == true
    else { return }
    peeked[window.id] = Peeked(
      original: window.frame.origin, moved: origin, until: .now + Self.peekLasts)
    restoringPeeks = restoringPeeks ?? Task { await restorePeeks() }
    try? await Task.sleep(for: .milliseconds(500))
  }

  private func restorePeeks() async {
    while !peeked.isEmpty {
      try? await Task.sleep(for: .seconds(5))
      for (id, peek) in peeked where peek.until <= .now {
        peeked[id] = nil
        guard let window = WindowCatalog.window(id), window.frame.origin == peek.moved else {
          continue
        }
        WindowCatalog.element(of: window)?.move(to: peek.original)
      }
    }
    restoringPeeks = nil
  }

  private func resolve(_ ref: String) throws -> Window {
    guard let id = CGWindowID(ref), let window = WindowCatalog.window(id) else {
      throw ComputerError.windowGone
    }
    return window
  }

  private func kind(of pid: pid_t) -> AppKind {
    if let known = kinds[pid] { return known }
    let kind = AppKind.of(pid)
    kinds[pid] = kind
    return kind
  }

  private func screenPoint(_ point: CGPoint, in window: Window) throws -> CGPoint {
    try Self.screenPoint(point, in: window, imageSize: imageSizes[window.id])
  }

  private static func screenPoint(_ point: CGPoint, in window: Window, imageSize: CGSize?) throws
    -> CGPoint
  {
    let image = imageSize ?? window.frame.size
    guard (0...image.width).contains(point.x), (0...image.height).contains(point.y) else {
      throw ComputerError.outsideWindow
    }
    return CGPoint(
      x: window.frame.minX + point.x * window.frame.width / image.width,
      y: window.frame.minY + point.y * window.frame.height / image.height)
  }

  private func summary(of window: Window, isFront: Bool) -> WindowSummary {
    WindowSummary(
      ref: String(window.id), app: window.app, title: window.title, isFront: isFront,
      isMinimized: !window.isOnScreen)
  }

  @MainActor
  private static func open(_ url: URL, activates: Bool) async throws -> pid_t {
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.activates = activates
    let app =
      url.pathExtension == "app"
      ? try await NSWorkspace.shared.openApplication(at: url, configuration: configuration)
      : try await NSWorkspace.shared.open(url, configuration: configuration)
    return app.processIdentifier
  }

  private static func screens() -> [CGRect] {
    var count: UInt32 = 0
    guard CGGetActiveDisplayList(0, nil, &count) == .success, count > 0 else { return [] }
    var displays = [CGDirectDisplayID](repeating: 0, count: Int(count))
    guard CGGetActiveDisplayList(count, &displays, &count) == .success else { return [] }
    return displays.prefix(Int(count)).map(CGDisplayBounds)
  }
}

extension OpenTarget {
  fileprivate var name: String {
    switch self {
    case .app(let name): name
    case .file(let path): (path as NSString).lastPathComponent
    case .url(let url): url.host() ?? url.absoluteString
    }
  }

  fileprivate func url() throws -> URL {
    switch self {
    case .app(let name):
      guard let url = Self.application(named: name) else { throw ComputerError.appNotFound(name) }
      return url
    case .file(let path):
      return URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
    case .url(let url):
      return url
    }
  }

  private static func application(named name: String) -> URL? {
    let folders = [
      "/Applications", "/Applications/Utilities", "/System/Applications",
      "/System/Applications/Utilities", NSHomeDirectory() + "/Applications",
    ]
    let wanted = name.lowercased().replacing(/\.app$/, with: "")
    for folder in folders {
      let names = (try? FileManager.default.contentsOfDirectory(atPath: folder)) ?? []
      if let match = names.first(where: { $0.lowercased() == wanted + ".app" }) {
        return URL(fileURLWithPath: folder).appending(path: match)
      }
    }
    return nil
  }
}
