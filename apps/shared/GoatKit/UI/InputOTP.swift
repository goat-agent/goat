import SwiftUI

/// web's `InputOTP`: one text field whose text is never seen, drawn as a cell per digit, so the
/// keyboard's code suggestion, pasting, and VoiceOver work as in any field. It takes focus when
/// it appears and whenever it is enabled again, since a code is all its screen asks for, and
/// dims while it is disabled.
struct InputOTP: View {
  @Binding var code: String
  var length = 6

  @FocusState private var isFocused: Bool
  @Environment(\.isEnabled) private var isEnabled

  #if os(macOS)
    private let cellHeight: CGFloat = 52
  #else
    private let cellHeight: CGFloat = 56
  #endif

  var body: some View {
    ZStack(alignment: .leading) {
      TextField("Code", text: $code)
        .textContentType(.oneTimeCode)
        #if os(iOS)
          .keyboardType(.numberPad)
        #endif
        .focused($isFocused)
        .foregroundStyle(.clear)
        .tint(.clear)
      HStack(spacing: 8) {
        ForEach(0..<length, id: \.self) { index in
          cell(index)
        }
      }
      .accessibilityHidden(true)
      .allowsHitTesting(false)
      .opacity(isEnabled ? 1 : 0.5)
    }
    .contentShape(.rect)
    .onTapGesture { isFocused = true }
    .onAppear { isFocused = true }
    .onChange(of: isEnabled) { if isEnabled { isFocused = true } }
    .onChange(of: code) { _, newValue in
      let digits = String(newValue.filter(\.isNumber).prefix(length))
      if digits != newValue { code = digits }
    }
  }

  private func cell(_ index: Int) -> some View {
    let characters = Array(code)
    let isCurrent = isFocused && index == min(characters.count, length - 1)
    let shape = RoundedRectangle(cornerRadius: 14, style: .continuous)
    return Text(index < characters.count ? String(characters[index]) : "")
      .font(.system(size: 22, weight: .semibold, design: .rounded))
      .monospacedDigit()
      .foregroundStyle(Palette.foreground)
      .frame(maxWidth: .infinity)
      .frame(height: cellHeight)
      .background(isCurrent ? Palette.background : Palette.input, in: shape)
      .overlay { shape.strokeBorder(Palette.ring, lineWidth: 1.5).opacity(isCurrent ? 1 : 0) }
  }
}

#Preview {
  @Previewable @State var code = "481"
  InputOTP(code: $code)
    .padding(24)
    .background(Palette.background)
}
