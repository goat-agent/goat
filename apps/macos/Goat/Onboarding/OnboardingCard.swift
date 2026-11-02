import SwiftUI

/// One step of getting started: a title, an optional line under it, the fields, and the step's
/// one next action, which the Return key presses. A step that submits by itself has none.
struct OnboardingCard<Fields: View, Links: View>: View {
  let title: LocalizedStringKey
  var message: Text?
  var action: LocalizedStringKey?
  var workingAction: LocalizedStringKey = ""
  let isWorking: Bool
  var canSubmit = true
  let error: String?
  var submit: () -> Void = {}
  @ViewBuilder let fields: Fields
  @ViewBuilder let links: Links

  var body: some View {
    VStack(alignment: .leading, spacing: 0) {
      Text(title)
        .font(.system(size: 24, weight: .bold))
        .foregroundStyle(Palette.foreground)
      if let message {
        message
          .foregroundStyle(Palette.mutedForeground)
          .fixedSize(horizontal: false, vertical: true)
          .padding(.top, 4)
      }
      VStack(alignment: .leading, spacing: 8) { fields }
        .controlSize(.large)
        .padding(.top, 20)
      if let error {
        FormFooter(hint: "", error: error)
          .padding(.top, 8)
          .transition(.opacity)
      }
      if let action {
        Button(action: submit) {
          Text(isWorking ? workingAction : action)
            .frame(maxWidth: .infinity)
        }
        .buttonStyle(.primary)
        .controlSize(.large)
        .keyboardShortcut(.defaultAction)
        .disabled(!canSubmit || isWorking)
        .padding(.top, 10)
      }
      HStack { links }
        .buttonStyle(.inline)
        .controlSize(.small)
        .padding(.top, 14)
    }
    .frame(width: 316)
    .padding(.horizontal, 52)
    .padding(.vertical, 48)
    .animation(.default, value: error)
  }
}

extension OnboardingCard where Links == EmptyView {
  init(
    title: LocalizedStringKey, message: Text? = nil, action: LocalizedStringKey? = nil,
    workingAction: LocalizedStringKey = "", isWorking: Bool, canSubmit: Bool = true,
    error: String?, submit: @escaping () -> Void = {}, @ViewBuilder fields: () -> Fields
  ) {
    self.init(
      title: title, message: message, action: action, workingAction: workingAction,
      isWorking: isWorking, canSubmit: canSubmit, error: error, submit: submit, fields: fields,
      links: { EmptyView() })
  }
}
