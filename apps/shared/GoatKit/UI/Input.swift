import SwiftUI

extension View {
  /// web's `Input`, and its `Textarea` when the text field grows vertically: a filled field with
  /// no border until it has focus. A taller field keeps the single-line corner.
  func inputStyle() -> some View {
    modifier(InputStyle())
  }
}

private struct InputStyle: ViewModifier {
  @Environment(\.controlSize) private var controlSize
  @FocusState private var isFocused: Bool

  func body(content: Content) -> some View {
    let shape = RoundedRectangle(cornerRadius: controlSize.height / 2, style: .continuous)
    content
      .textFieldStyle(.plain)
      .font(controlSize.inputFont)
      .foregroundStyle(Palette.foreground)
      .focused($isFocused)
      .padding(.horizontal, controlSize.height * 0.42)
      .padding(.vertical, controlSize.height * 0.25)
      .frame(minHeight: controlSize.height)
      .background(isFocused ? Palette.background : Palette.input, in: shape)
      .overlay { shape.strokeBorder(Palette.ring, lineWidth: 1.5).opacity(isFocused ? 1 : 0) }
      .contentShape(shape)
      .onTapGesture { isFocused = true }
  }
}

/// web's `Label` for a field: a quiet name above it, lined up with the text inside.
struct FieldLabel: View {
  let title: LocalizedStringKey

  init(_ title: LocalizedStringKey) {
    self.title = title
  }

  @Environment(\.controlSize) private var controlSize

  var body: some View {
    Text(title)
      .font(.callout)
      .foregroundStyle(Palette.mutedForeground)
      .padding(.leading, controlSize.height * 0.42)
  }
}

#Preview {
  @Previewable @State var email = ""
  @Previewable @State var instructions = ""
  VStack(alignment: .leading, spacing: 12) {
    FieldLabel("Email")
    TextField("Email", text: $email, prompt: Text(verbatim: "you@example.com"))
      .inputStyle()
    TextField("Instructions", text: $instructions, axis: .vertical)
      .lineLimit(4...8)
      .inputStyle()
    TextField("Email", text: $email).inputStyle().controlSize(.large)
  }
  .padding(24)
  .frame(width: 360)
  .background(Palette.background)
}
