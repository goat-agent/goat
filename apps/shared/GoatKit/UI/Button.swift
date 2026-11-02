import SwiftUI

/// web's `Button` variants as button styles, so screens keep SwiftUI's `Button` with its roles,
/// keyboard shortcuts, and accessibility: `.buttonStyle(.primary)`, `.controlSize(.small)`.
struct FilledButtonStyle: ButtonStyle {
  enum Variant {
    /// The one next action on a screen.
    case primary
    case secondary
    case destructive
    case ghost
    /// Text alone, lined up with the text around it, as web's `link` variant. SwiftUI already
    /// names a style `.link`.
    case inline
  }

  let variant: Variant

  func makeBody(configuration: Configuration) -> some View {
    FilledButton(variant: variant, configuration: configuration)
  }
}

private struct FilledButton: View {
  let variant: FilledButtonStyle.Variant
  let configuration: ButtonStyleConfiguration

  @Environment(\.controlSize) private var controlSize
  @Environment(\.isEnabled) private var isEnabled

  var body: some View {
    configuration.label
      .font(controlSize.font)
      .lineLimit(1)
      .foregroundStyle(foreground)
      .padding(.horizontal, horizontalPadding)
      .frame(minHeight: variant == .inline ? nil : controlSize.height)
      .background(background, in: .capsule)
      .contentShape(.capsule)
      .opacity(configuration.isPressed ? 0.75 : 1)
  }

  private var horizontalPadding: CGFloat {
    switch variant {
    case .ghost: 8
    case .inline: 0
    default: controlSize.height / 2
    }
  }

  private var foreground: Color {
    guard isEnabled else { return Palette.mutedForeground }
    switch variant {
    case .primary: return Palette.primaryForeground
    case .secondary: return Palette.foreground
    case .destructive: return Palette.destructive
    case .ghost, .inline: return Palette.mutedForeground
    }
  }

  private var background: Color {
    switch variant {
    case .primary: isEnabled ? Palette.primary : Palette.secondary
    case .secondary, .destructive: Palette.secondary
    case .ghost, .inline: .clear
    }
  }
}

/// web's `Button variant="ghost" size="icon"`: an icon in a circle that lights up under the
/// pointer.
struct IconButtonStyle: ButtonStyle {
  func makeBody(configuration: Configuration) -> some View {
    IconButton(configuration: configuration)
  }
}

private struct IconButton: View {
  let configuration: ButtonStyleConfiguration

  @Environment(\.controlSize) private var controlSize
  @State private var isHovered = false

  private var side: CGFloat {
    #if os(macOS)
      28
    #else
      controlSize.height
    #endif
  }

  var body: some View {
    configuration.label
      .labelStyle(.iconOnly)
      .foregroundStyle(isHovered ? Palette.foreground : Palette.mutedForeground)
      .frame(width: side, height: side)
      .background(isHovered ? Palette.accent : .clear, in: .circle)
      .contentShape(.circle)
      .opacity(configuration.isPressed ? 0.6 : 1)
      .onHover { isHovered = $0 }
      .animation(.easeOut(duration: 0.12), value: isHovered)
  }
}

extension ButtonStyle where Self == FilledButtonStyle {
  static var primary: Self { .init(variant: .primary) }
  static var secondary: Self { .init(variant: .secondary) }
  static var destructive: Self { .init(variant: .destructive) }
  static var ghost: Self { .init(variant: .ghost) }
  static var inline: Self { .init(variant: .inline) }
}

extension ButtonStyle where Self == IconButtonStyle {
  static var icon: Self { .init() }
}

#Preview {
  VStack(alignment: .leading, spacing: 12) {
    HStack {
      Button("Continue") {}.buttonStyle(.primary)
      Button("Add Integration") {}.buttonStyle(.secondary)
      Button("Sign Out") {}.buttonStyle(.destructive)
      Button("Use a Different Email") {}.buttonStyle(.ghost)
      Button("Continue") {}.buttonStyle(.primary).disabled(true)
    }
    HStack {
      Button("Remove") {}.buttonStyle(.secondary)
      Button("Reconnect") {}.buttonStyle(.primary)
    }
    .controlSize(.small)
    Button {
    } label: {
      Text("Continue").frame(maxWidth: .infinity)
    }
    .buttonStyle(.primary)
    .controlSize(.large)
    HStack {
      Button("Search", systemImage: "magnifyingglass") {}
      Button("New Agent", systemImage: "plus") {}
    }
    .buttonStyle(.icon)
  }
  .padding(24)
  .background(Palette.background)
}
