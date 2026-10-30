import AppKit
import SwiftUI

/// Shows that the agent has the user's pointer: a soft ring with Goat's face follows the
/// pointer, since that is where the eye goes when it moves by itself, and a capsule at the top
/// says how to stop. Both panels ignore clicks, and Esc ends the borrow.
final class BorrowIndicator {
  private static let haloSize: CGFloat = 52

  private var capsule: NSPanel?
  private var halo: NSPanel?
  private var following: Task<Void, Never>?
  private var escapeMonitor: Any?

  func show(onEscape: @escaping () -> Void) {
    guard capsule == nil, let screen = NSScreen.main else { return }

    let capsule = Self.panel(BorrowCapsule(), size: CGSize(width: 260, height: 44))
    let frame = screen.visibleFrame
    capsule.setFrameOrigin(
      CGPoint(x: frame.midX - capsule.frame.width / 2, y: frame.maxY - capsule.frame.height - 10))
    capsule.orderFrontRegardless()
    self.capsule = capsule

    let halo = Self.panel(
      BorrowHalo(), size: CGSize(width: Self.haloSize, height: Self.haloSize))
    halo.orderFrontRegardless()
    self.halo = halo
    following = Task { [weak halo] in
      while !Task.isCancelled, let halo {
        let point = NSEvent.mouseLocation
        halo.setFrameOrigin(CGPoint(x: point.x - Self.haloSize / 2, y: point.y - Self.haloSize / 2))
        try? await Task.sleep(for: .milliseconds(16))
      }
    }

    escapeMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { event in
      if event.keyCode == 53 { onEscape() }
    }
  }

  func hide() {
    following?.cancel()
    following = nil
    capsule?.orderOut(nil)
    halo?.orderOut(nil)
    capsule = nil
    halo = nil
    if let escapeMonitor { NSEvent.removeMonitor(escapeMonitor) }
    escapeMonitor = nil
  }

  private static func panel(_ view: some View, size: CGSize) -> NSPanel {
    let panel = NSPanel(
      contentRect: CGRect(origin: .zero, size: size),
      styleMask: [.borderless, .nonactivatingPanel],
      backing: .buffered,
      defer: false
    )
    panel.level = .popUpMenu
    panel.ignoresMouseEvents = true
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = false
    panel.collectionBehavior = [
      .canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle,
    ]
    panel.contentView = NSHostingView(rootView: view)
    return panel
  }
}

private struct BorrowCapsule: View {
  var body: some View {
    HStack(spacing: 8) {
      AgentFace(size: 24)
      Text("Using your screen")
        .font(.callout.weight(.medium))
      Text(verbatim: "esc")
        .font(.caption)
        .foregroundStyle(.secondary)
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(.fill.tertiary, in: .rect(cornerRadius: 5))
        .accessibilityLabel("Press Escape to stop")
    }
    .padding(.horizontal, 10)
    .padding(.vertical, 6)
    .glassEffect(.regular, in: .capsule)
    .frame(maxWidth: .infinity, maxHeight: .infinity)
  }
}

private struct BorrowHalo: View {
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @State private var isBright = false

  var body: some View {
    ZStack {
      Circle()
        .fill(.tint.opacity(0.2))
        .frame(width: 28, height: 28)
        .overlay {
          Circle().stroke(.tint.opacity(isBright ? 1 : 0.7), lineWidth: 1.5)
        }
        .shadow(color: .accentColor.opacity(0.3), radius: 7)
      AgentFace(size: 18)
        .offset(x: 14, y: 14)
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity)
    .onAppear {
      guard !reduceMotion else { return }
      withAnimation(.easeInOut(duration: 1.2).repeatForever()) { isBright = true }
    }
  }
}

/// Goat's face on its usual yellow, for places that stand for the agent itself.
struct AgentFace: View {
  let size: CGFloat

  var body: some View {
    Image("Goat")
      .resizable()
      .scaledToFit()
      .padding(size * 0.08)
      .frame(width: size, height: size)
      .background(Color.yellow.opacity(0.85), in: .circle)
      .overlay { Circle().stroke(.white, lineWidth: size > 20 ? 0 : 1.5) }
      .accessibilityHidden(true)
  }
}
