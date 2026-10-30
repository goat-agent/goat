import AppKit
import ApplicationServices
import GoatAPI

/// Runs what the agent asks of this Mac. It works in the background through accessibility
/// whenever it can: pressing, setting values, choosing menu items, and typing reach one app without
/// touching the user's pointer or front app. Pointer gestures need the real pointer, so for those
/// it borrows the screen for a moment and gives it back.
public actor Computer {
  public typealias Outcome = Components.Schemas.ComputerOutcome
  public typealias View = Components.Schemas.ComputerView
  public typealias Target = Components.Schemas.ComputerTarget

  /// Told when a borrow starts, with the app's name, and when it ends, with nil.
  public typealias BorrowObserver = @Sendable (String?) async -> Void

  static let screenRef = "screen"
  private static let settle = Duration.milliseconds(350)
  private static let chromium: Set<String> = [
    "com.google.Chrome", "com.google.Chrome.canary", "com.microsoft.edgemac", "com.brave.Browser",
    "company.thebrowser.Browser", "com.vivaldi.Vivaldi", "com.operasoftware.Opera",
  ]

  private struct Geometry {
    /// The image the agent last saw of the window, in pixels.
    let imageSize: CGSize
  }

  private enum Resolved {
    case element(Elements.Entry)
    case point(CGPoint, window: WindowInfo?)
  }

  /// The element the agent named changed; nothing was done.
  private struct Stale: Error {
    let window: CGWindowID
  }

  private let elements = Elements()
  private let onBorrow: BorrowObserver
  private var geometry: [String: Geometry] = [:]
  private var lastRefs: [CGWindowID: Set<String>] = [:]
  private var preparedApps: Set<pid_t> = []
  private var isStopped = false

  public init(onBorrow: @escaping BorrowObserver) {
    self.onBorrow = onBorrow
  }

  /// Ends a borrow in progress; called when the user presses Esc or chooses Stop.
  public func stop() {
    isStopped = true
  }

  // MARK: Looking

  public func screen(includeImage: Bool) async throws -> Components.Schemas.ComputerScreen {
    let windows = WindowCatalog.all()
    let front = NSWorkspace.shared.frontmostApplication?.processIdentifier
    let frontWindow = windows.first { $0.pid == front && !$0.isMinimized }?.id
    var screen = Components.Schemas.ComputerScreen(
      ref: Self.screenRef,
      windows: windows.map {
        .init(
          ref: String($0.id), app: $0.app, title: $0.title, isFront: $0.id == frontWindow,
          isMinimized: $0.isMinimized)
      }
    )
    if includeImage {
      let bounds = CGDisplayBounds(CGMainDisplayID())
      let image = Capture.forModel(try await Capture.screen(), points: bounds.size)
      geometry[Self.screenRef] = Geometry(
        imageSize: CGSize(width: image.width, height: image.height))
      screen.image = try encode(image)
    }
    return screen
  }

  public func view(
    window ref: String, region: CGRect? = nil, waitSeconds: Double? = nil
  ) async throws -> View {
    if let waitSeconds, waitSeconds > 0 {
      try await Task.sleep(for: .seconds(min(waitSeconds, 60)))
    }
    if ref == Self.screenRef { return try await screenView(region: region) }
    guard let id = CGWindowID(ref), var info = WindowCatalog.window(id) else {
      throw ComputerError.windowGone
    }
    if info.isMinimized, let element = WindowCatalog.element(of: info) {
      element.set(kAXMinimizedAttribute, to: kCFBooleanFalse)
      try await Task.sleep(for: .milliseconds(400))
      info = WindowCatalog.window(id) ?? info
    }
    prepare(info.pid)

    let full = try await Capture.window(id)
    let header = Components.Schemas.ComputerWindow(ref: ref, app: info.app, title: info.title)
    if let region { return try zoom(full, region: region, ref: ref, header: header) }

    let image = Capture.forModel(full, points: info.frame.size)
    geometry[ref] = Geometry(imageSize: CGSize(width: image.width, height: image.height))
    var nodes: [Components.Schemas.ComputerNode] = []
    var isTruncated = false
    // Apps build their accessibility tree on demand, so a window that just opened can read empty.
    for attempt in 0..<4 {
      if attempt > 0 { try await Task.sleep(for: .milliseconds(250)) }
      guard let element = WindowCatalog.element(of: info) else { continue }
      let snapshot = Snapshot(window: element, id: id, frame: info.frame, elements: elements)
      guard !snapshot.nodes.isEmpty else { continue }
      nodes = snapshot.nodes
      isTruncated = snapshot.isTruncated
      elements.keep(snapshot.refs.union(lastRefs[id] ?? []), in: id)
      lastRefs[id] = snapshot.refs
      break
    }
    return View(
      window: header, image: try encode(image), nodes: nodes,
      isTruncated: isTruncated ? true : nil)
  }

  // MARK: Acting

  public func click(
    _ target: Target, button: String?, count: Int?, modifiers: [String]
  ) async throws -> Outcome {
    try await act {
      let resolved = try resolve(target)
      let flags = Input.flags(modifiers)
      let clicks = count ?? 1
      if case .element(let entry) = resolved, flags.isEmpty, clicks == 1 {
        let actions = entry.element.actions()
        if button == nil || button == "left" {
          if actions.contains(kAXPressAction), entry.element.perform(kAXPressAction) {
            return entry.window
          }
          if entry.element.isSettable(kAXSelectedAttribute),
            entry.element.set(kAXSelectedAttribute, to: kCFBooleanTrue)
          {
            return entry.window
          }
        } else if button == "right", actions.contains(kAXShowMenuAction),
          entry.element.perform(kAXShowMenuAction)
        {
          return entry.window
        }
      }
      let (point, pid, window) = try locate(resolved)
      let mouseButton: CGMouseButton =
        switch button {
        case "right": .right
        case "middle": .center
        default: .left
        }
      try await borrow(pid: pid, window: window) {
        await Input.click(at: point, button: mouseButton, count: clicks, flags: flags)
      }
      return window
    }
  }

  public func hover(_ target: Target) async throws -> Outcome {
    let (point, pid, window) = try locate(try resolve(target))
    // What hovering reveals goes away with the pointer, so the look happens before giving it back.
    let view = try await borrow(pid: pid, window: window) { () async throws -> View? in
      Input.move(to: point)
      try await Task.sleep(for: .milliseconds(600))
      guard let window else { return nil }
      return try await self.view(window: String(window))
    }
    return Outcome(view: view)
  }

  public func drag(_ from: Target, to: Target, modifiers: [String]) async throws -> Outcome {
    try await act {
      let (start, pid, window) = try locate(try resolve(from))
      let (end, _, _) = try locate(try resolve(to))
      try await borrow(pid: pid, window: window) {
        await Input.drag(from: start, to: end, flags: Input.flags(modifiers))
      }
      return window
    }
  }

  public func scroll(_ target: Target, direction: String, amount: Int) async throws -> Outcome {
    try await act {
      let (point, pid, window) = try locate(try resolve(target))
      let notches = Int32(clamping: max(1, amount))
      let (dx, dy): (Int32, Int32) =
        switch direction {
        case "up": (0, notches)
        case "left": (notches, 0)
        case "right": (-notches, 0)
        default: (0, -notches)
        }
      try await borrow(pid: pid, window: window) {
        await Input.scroll(at: point, dx: dx, dy: dy)
      }
      return window
    }
  }

  public func type(_ target: Target, text: String) async throws -> Outcome {
    try await act {
      switch try resolve(target) {
      case .element(let entry):
        entry.element.set(kAXFocusedAttribute, to: kCFBooleanTrue)
        // Inserting at the caret through accessibility needs no keyboard focus at all.
        if entry.element.isSettable(kAXSelectedTextAttribute),
          entry.element.set(kAXSelectedTextAttribute, to: text as CFString)
        {
          return entry.window
        }
        guard let pid = entry.element.pid else { throw ComputerError.windowGone }
        await Input.type(text, to: .app(pid))
        return entry.window
      case .point(let point, let window) where target.x != nil:
        guard let window else { throw ComputerError.windowGone }
        try await borrow(pid: window.pid, window: window.id) {
          await Input.click(at: point, button: .left, count: 1, flags: [])
          await Input.type(text, to: .system)
        }
        return window.id
      case .point(_, let window):
        guard let window else { throw ComputerError.windowGone }
        await Input.type(text, to: .app(window.pid))
        return window.id
      }
    }
  }

  public func key(_ target: Target, keys: String, repeating times: Int?) async throws -> Outcome {
    let chord = try Input.chord(keys)
    return try await act {
      switch try resolve(target) {
      case .element(let entry):
        entry.element.set(kAXFocusedAttribute, to: kCFBooleanTrue)
        guard let pid = entry.element.pid else { throw ComputerError.windowGone }
        await Input.press(chord, times: times ?? 1, to: .app(pid))
        return entry.window
      case .point(_, let window):
        guard let window else { throw ComputerError.windowGone }
        await Input.press(chord, times: times ?? 1, to: .app(window.pid))
        return window.id
      }
    }
  }

  public func set(_ target: Target, value: String) async throws -> Outcome {
    try await act {
      guard case .element(let entry) = try resolve(target) else { throw ComputerError.notAnElement }
      try Values.set(entry.element, role: entry.role, to: value)
      return entry.window
    }
  }

  public func menu(window ref: String, path: [String]) async throws -> Outcome {
    try await act {
      guard let id = CGWindowID(ref), let info = WindowCatalog.window(id) else {
        throw ComputerError.windowGone
      }
      try Menus.choose(path, in: info.pid)
      return id
    }
  }

  public func open(app: String?, path: String?, url: String?) async throws -> Outcome {
    try await act {
      let pid = try await Opening.open(app: app, path: path, url: url)
      if let window = try await Opening.firstWindow(of: pid, within: .milliseconds(1500)) {
        return window
      }
      // Some apps, Calculator among them, make their first window only when they come to the
      // front; bring it forward for a moment, and the window stays when the user's app returns.
      return try await borrow(pid: pid, window: nil) {
        try await Opening.firstWindow(of: pid, within: .seconds(4))
      }
    }
  }

  public func clipboard(text: String?, path: String?) -> Outcome {
    if text != nil || path != nil {
      Opening.copy(text: text, path: path)
      return Outcome()
    }
    let contents = Opening.pasteboard()
    return Outcome(clipboard: .init(text: contents.text, files: contents.files))
  }

  // MARK: Plumbing

  /// Runs an action, lets the app settle, and looks at the window it changed. A stale element
  /// becomes an outcome that says so, with the window as it is now.
  private func act(_ work: () async throws -> CGWindowID?) async throws -> Outcome {
    do {
      let window = try await work()
      try await Task.sleep(for: Self.settle)
      guard let window, WindowCatalog.window(window) != nil else { return Outcome() }
      return Outcome(view: try await view(window: String(window)))
    } catch let stale as Stale {
      return Outcome(view: try await view(window: String(stale.window)), isStale: true)
    }
  }

  private func resolve(_ target: Target) throws -> Resolved {
    if let ref = target.element {
      guard let entry = elements.entry(ref) else { throw ComputerError.unknownRef }
      if entry.element.isAlive { return .element(entry) }
      if let found = refind(entry) { return .element(found) }
      throw Stale(window: entry.window)
    }
    guard let ref = target.window else { throw ComputerError.unknownRef }
    let window = ref == Self.screenRef ? nil : CGWindowID(ref).flatMap(WindowCatalog.window)
    let frame =
      ref == Self.screenRef ? CGDisplayBounds(CGMainDisplayID()) : window?.frame
    guard let frame else { throw ComputerError.windowGone }
    guard let x = target.x, let y = target.y else {
      return .point(CGPoint(x: frame.midX, y: frame.midY), window: window)
    }
    // Points are pixels of the last full image of the window; the window may have moved since.
    let image = geometry[ref]?.imageSize ?? frame.size
    return .point(
      CGPoint(
        x: frame.minX + x * frame.width / image.width,
        y: frame.minY + y * frame.height / image.height),
      window: window)
  }

  /// The element again, when its app replaced it with an equal one at the same place.
  private func refind(_ entry: Elements.Entry) -> Elements.Entry? {
    guard let info = WindowCatalog.window(entry.window),
      var element = WindowCatalog.element(of: info)
    else { return nil }
    for index in entry.path {
      let children = element.children()
      guard index < children.count else { return nil }
      element = children[index]
    }
    let values = element.attributes([kAXRoleAttribute, kAXTitleAttribute, kAXDescriptionAttribute])
    let label =
      values[kAXTitleAttribute] as? String ?? values[kAXDescriptionAttribute] as? String
    guard values[kAXRoleAttribute] as? String == entry.role,
      (label ?? "").isEmpty ? entry.label == nil : label == entry.label
    else { return nil }
    return .init(
      element: element, window: entry.window, role: entry.role, label: entry.label,
      path: entry.path)
  }

  private func locate(_ resolved: Resolved) throws -> (CGPoint, pid_t, CGWindowID?) {
    switch resolved {
    case .element(let entry):
      guard let frame = entry.element.frame, let pid = entry.element.pid else {
        throw Stale(window: entry.window)
      }
      return (CGPoint(x: frame.midX, y: frame.midY), pid, entry.window)
    case .point(let point, let window):
      if let window { return (point, window.pid, window.id) }
      let front = NSWorkspace.shared.frontmostApplication?.processIdentifier ?? 0
      return (point, front, nil)
    }
  }

  /// Takes the user's pointer and front app for `work` and gives both back afterwards.
  private func borrow<T>(
    pid: pid_t, window: CGWindowID?, _ work: () async throws -> T
  ) async throws -> T {
    isStopped = false
    let app = NSRunningApplication(processIdentifier: pid)
    await onBorrow(app?.localizedName ?? "")
    let previousApp = NSWorkspace.shared.frontmostApplication
    let previousPoint = CGEvent(source: nil)?.location
    defer {
      if let previousPoint { CGWarpMouseCursorPosition(previousPoint) }
      if let previous = previousApp?.processIdentifier, previous != pid {
        AXUIElement.application(previous).set(kAXFrontmostAttribute, to: kCFBooleanTrue)
      }
    }

    // Since macOS 14 an app in the background cannot activate another one; accessibility can,
    // as window managers do.
    AXUIElement.application(pid).set(kAXFrontmostAttribute, to: kCFBooleanTrue)
    if let window, let info = WindowCatalog.window(window) {
      WindowCatalog.element(of: info)?.perform(kAXRaiseAction)
    }
    try await Task.sleep(for: .milliseconds(150))
    do {
      guard !isStopped else { throw ComputerError.stopped }
      let result = try await work()
      guard !isStopped else { throw ComputerError.stopped }
      await onBorrow(nil)
      return result
    } catch {
      await onBorrow(nil)
      throw error
    }
  }

  /// Turns on the accessibility trees that Chromium and Electron apps build only on request.
  private func prepare(_ pid: pid_t) {
    guard preparedApps.insert(pid).inserted else { return }
    let app = AXUIElement.application(pid)
    app.set("AXManualAccessibility", to: kCFBooleanTrue)
    if let id = NSRunningApplication(processIdentifier: pid)?.bundleIdentifier,
      Self.chromium.contains(id)
    {
      app.set("AXEnhancedUserInterface", to: kCFBooleanTrue)
    }
  }

  private func screenView(region: CGRect?) async throws -> View {
    let bounds = CGDisplayBounds(CGMainDisplayID())
    let full = try await Capture.screen()
    let header = Components.Schemas.ComputerWindow(ref: Self.screenRef, app: "", title: "Screen")
    if let region { return try zoom(full, region: region, ref: Self.screenRef, header: header) }
    let image = Capture.forModel(full, points: bounds.size)
    geometry[Self.screenRef] = Geometry(imageSize: CGSize(width: image.width, height: image.height))
    return View(window: header, image: try encode(image), nodes: [])
  }

  /// Part of the window at full resolution. Points stay in the last full image's pixels.
  private func zoom(
    _ full: CGImage, region: CGRect, ref: String, header: Components.Schemas.ComputerWindow
  ) throws -> View {
    let shown = geometry[ref]?.imageSize ?? CGSize(width: full.width, height: full.height)
    let scale = CGFloat(full.width) / shown.width
    let rect = CGRect(
      x: region.minX * scale, y: region.minY * scale, width: region.width * scale,
      height: region.height * scale)
    guard let cropped = Capture.crop(full, to: rect) else { throw ComputerError.windowGone }
    let image = Capture.forModel(
      cropped, points: CGSize(width: cropped.width, height: cropped.height))
    return View(window: header, image: try encode(image), nodes: [])
  }

  private func encode(_ image: CGImage) throws -> Components.Schemas.ComputerImage {
    guard let data = Capture.jpeg(image) else { throw ComputerError.windowGone }
    return .init(data: data, width: image.width, height: image.height)
  }
}
