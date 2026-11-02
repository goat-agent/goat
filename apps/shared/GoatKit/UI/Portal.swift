import SwiftUI

extension View {
  /// Where dropdown menus and alert dialogs are drawn, over everything else, as web draws them
  /// in a portal, so one opened deep in a narrow panel is not clipped by it. A window's root view
  /// and every sheet's root view need one.
  func portalHost() -> some View {
    modifier(PortalHost())
  }

  /// web's `DropdownMenu`, opening from this view. Choosing an item closes it, as do a click
  /// outside and Escape.
  func dropdownMenu<Menu: View>(
    isPresented: Binding<Bool>, placement: PortalPlacement = .below(.leading),
    @ViewBuilder content: @escaping () -> Menu
  ) -> some View {
    modifier(DropdownPresenter(isPresented: isPresented, placement: placement, menu: content))
  }

  /// A dialog over the whole window, behind which everything dims and blurs.
  func portalDialog<Dialog: View>(
    isPresented: Binding<Bool>, @ViewBuilder content: @escaping () -> Dialog
  ) -> some View {
    modifier(DialogPresenter(isPresented: isPresented, dialog: content))
  }
}

enum PortalPlacement: Equatable {
  enum Edge { case leading, trailing }
  case above(Edge)
  case below(Edge)
}

/// Closes the dropdown menu around the view that reads it, as `DismissAction` closes a sheet.
struct DismissDropdownMenuAction {
  fileprivate var close: () -> Void = {}

  func callAsFunction() { close() }
}

extension EnvironmentValues {
  @Entry var dismissDropdownMenu = DismissDropdownMenuAction()
}

nonisolated struct PortalRequest: Identifiable, Equatable {
  let id: UUID
  let anchor: Anchor<CGRect>?
  let placement: PortalPlacement?
  let content: AnyView
  let dismiss: () -> Void

  static func == (lhs: Self, rhs: Self) -> Bool {
    lhs.id == rhs.id && lhs.anchor == rhs.anchor
  }
}

nonisolated private struct PortalKey: PreferenceKey {
  static var defaultValue: [PortalRequest] { [] }

  static func reduce(value: inout [PortalRequest], nextValue: () -> [PortalRequest]) {
    value += nextValue()
  }
}

private struct PortalHost: ViewModifier {
  @State private var isShowingDialog = false

  func body(content: Content) -> some View {
    content
      .overlay {
        if isShowingDialog {
          Backdrop()
            .ignoresSafeArea()
            .allowsHitTesting(false)
            .transition(.opacity)
        }
      }
      .overlayPreferenceValue(PortalKey.self) { requests in
        GeometryReader { proxy in
          ZStack {
            ForEach(requests) { request in
              PortalLayer(
                request: request, bounds: proxy.size, anchor: request.anchor.map { proxy[$0] })
            }
          }
        }
      }
      .onPreferenceChange(PortalKey.self) { requests in
        isShowingDialog = requests.contains { $0.anchor == nil }
      }
      .animation(.smooth(duration: 0.2), value: isShowingDialog)
      // A host draws what was opened inside it, so an outer host must not draw it again.
      .preference(key: PortalKey.self, value: [])
  }
}

private struct DropdownPresenter<Menu: View>: ViewModifier {
  @Binding var isPresented: Bool
  let placement: PortalPlacement
  let menu: () -> Menu

  @State private var id = UUID()

  func body(content: Content) -> some View {
    content.transformAnchorPreference(key: PortalKey.self, value: .bounds) { requests, anchor in
      guard isPresented else { return }
      let dismiss = { isPresented = false }
      requests.append(
        PortalRequest(
          id: id, anchor: anchor, placement: placement,
          content: AnyView(
            DropdownMenu(content: menu).environment(
              \.dismissDropdownMenu, DismissDropdownMenuAction(close: dismiss))),
          dismiss: dismiss))
    }
  }
}

private struct DialogPresenter<Dialog: View>: ViewModifier {
  @Binding var isPresented: Bool
  let dialog: () -> Dialog

  @State private var id = UUID()

  func body(content: Content) -> some View {
    content.transformPreference(PortalKey.self) { requests in
      guard isPresented else { return }
      requests.append(
        PortalRequest(
          id: id, anchor: nil, placement: nil, content: AnyView(dialog()),
          dismiss: { isPresented = false }))
    }
  }
}

/// What a dialog puts between itself and the window: a blur and a tint, as web's overlay does.
/// The system draws the blur from what the window shows, because a SwiftUI filter on the content
/// cannot render the AppKit and UIKit views inside it, and on the Mac it also stops clicks above
/// the safe area.
private struct Backdrop: View {
  var body: some View {
    #if os(macOS)
      WindowBlur()
        .overlay(Color.black.opacity(0.1))
    #else
      Rectangle()
        .fill(.ultraThinMaterial)
    #endif
  }
}

#if os(macOS)
  private struct WindowBlur: NSViewRepresentable {
    func makeNSView(context: Context) -> NSVisualEffectView {
      let view = NSVisualEffectView()
      view.blendingMode = .withinWindow
      view.material = .windowBackground
      view.state = .active
      // The system's blur has one strength, and at full strength it hides the window and lets
      // the wallpaper's color through; part of it softens the window while it stays readable.
      view.alphaValue = 0.6
      return view
    }

    func updateNSView(_ view: NSVisualEffectView, context: Context) {}
  }
#endif

private struct PortalLayer: View {
  let request: PortalRequest
  let bounds: CGSize
  let anchor: CGRect?

  @FocusState private var isFocused: Bool

  private static let gap: CGFloat = 6
  /// A dialog keeps this far from the window's edges: clear of the Mac's window buttons, which
  /// take the top 32 points, and the same on every side.
  private static let dialogMargin: CGFloat = 40

  var body: some View {
    ZStack(alignment: .topLeading) {
      Color.clear
        .contentShape(.rect)
        .onTapGesture(perform: request.dismiss)
        .ignoresSafeArea()
      placed
        .focusable()
        .focusEffectDisabled()
        .focused($isFocused)
        .onAppear { isFocused = true }
        #if os(macOS)
          .onExitCommand(perform: request.dismiss)
        #endif
    }
    .frame(width: bounds.width, height: bounds.height)
  }

  @ViewBuilder private var placed: some View {
    if let anchor, let placement = request.placement {
      switch placement {
      case .above(let edge):
        request.content
          .padding(edgeInsets(edge, anchor))
          .frame(
            width: bounds.width, height: max(anchor.minY - Self.gap, 0),
            alignment: frameAlignment(edge, bottom: true))
      case .below(let edge):
        request.content
          .padding(edgeInsets(edge, anchor))
          .frame(
            width: bounds.width, height: max(bounds.height - anchor.maxY - Self.gap, 0),
            alignment: frameAlignment(edge, bottom: false)
          )
          .offset(y: anchor.maxY + Self.gap)
      }
    } else {
      request.content
        .padding(Self.dialogMargin)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .ignoresSafeArea(.container)
    }
  }

  private func edgeInsets(_ edge: PortalPlacement.Edge, _ anchor: CGRect) -> EdgeInsets {
    switch edge {
    case .leading: EdgeInsets(top: 0, leading: anchor.minX, bottom: 0, trailing: 0)
    case .trailing: EdgeInsets(top: 0, leading: 0, bottom: 0, trailing: bounds.width - anchor.maxX)
    }
  }

  private func frameAlignment(_ edge: PortalPlacement.Edge, bottom: Bool) -> Alignment {
    switch (edge, bottom) {
    case (.leading, true): .bottomLeading
    case (.trailing, true): .bottomTrailing
    case (.leading, false): .topLeading
    case (.trailing, false): .topTrailing
    }
  }
}
