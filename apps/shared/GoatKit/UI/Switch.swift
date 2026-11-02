import SwiftUI

/// web's `Switch`: a `Toggle` whose track is the foreground color when on, so the only color on a
/// settings screen stays the one next action.
struct Switch<Label: View>: View {
  @Binding var isOn: Bool
  @ViewBuilder let label: Label

  var body: some View {
    Toggle(isOn: $isOn) { label }
      .toggleStyle(TrackToggleStyle())
  }
}

extension Switch where Label == Text {
  init(_ title: LocalizedStringKey, isOn: Binding<Bool>) {
    self.init(isOn: isOn) { Text(title) }
  }
}

private struct TrackToggleStyle: ToggleStyle {
  /// As web's switch: the thumb is the background's color, except off in dark mode, where that
  /// would vanish into the track.
  private static let offThumb = Color(light: Palette.background, dark: Palette.foreground)

  #if os(macOS)
    private let size = CGSize(width: 34, height: 20)
  #else
    private let size = CGSize(width: 48, height: 28)
  #endif

  func makeBody(configuration: Configuration) -> some View {
    HStack(spacing: 12) {
      configuration.label
        .frame(maxWidth: .infinity, alignment: .leading)
      Button {
        withAnimation(.snappy(duration: 0.2)) { configuration.isOn.toggle() }
      } label: {
        Capsule()
          .fill(configuration.isOn ? Palette.foreground : Palette.muted)
          .frame(width: size.width, height: size.height)
          .overlay(alignment: configuration.isOn ? .trailing : .leading) {
            Circle()
              .fill(configuration.isOn ? Palette.background : Self.offThumb)
              .padding(2)
              .shadow(color: .black.opacity(0.25), radius: 1, y: 1)
          }
      }
      .buttonStyle(.plain)
    }
    .accessibilityElement(children: .combine)
    .accessibilityAddTraits(.isToggle)
    .accessibilityValue(configuration.isOn ? Text("On") : Text("Off"))
  }
}

#Preview {
  @Previewable @State var isOn = true
  Switch("Let the agent use this Mac", isOn: $isOn)
    .padding(24)
    .frame(width: 360)
    .background(Palette.background)
}
