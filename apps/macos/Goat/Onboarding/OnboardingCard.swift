import SwiftUI

/// One step of getting started, laid out the way Mac sign-in windows are: the agent's face, a
/// title and a line of explanation, the fields, and one button that the Return key presses.
struct OnboardingCard<Fields: View, Links: View>: View {
  let title: LocalizedStringKey
  var message: Text?
  let action: LocalizedStringKey
  let workingAction: LocalizedStringKey
  let isWorking: Bool
  let canSubmit: Bool
  let error: String?
  let submit: () -> Void
  @ViewBuilder let fields: Fields
  @ViewBuilder let links: Links

  var body: some View {
    VStack(spacing: 0) {
      AgentFace(size: 64)
      Text(title)
        .font(.title2.weight(.semibold))
        .padding(.top, 18)
      if let message {
        message
          .foregroundStyle(.secondary)
          .multilineTextAlignment(.center)
          .fixedSize(horizontal: false, vertical: true)
          .padding(.top, 6)
      }

      VStack(alignment: .leading, spacing: 8) {
        fields
          .textFieldStyle(.roundedBorder)
          .controlSize(.large)
        if let error {
          Label(error, systemImage: "exclamationmark.circle.fill")
            .font(.callout)
            .foregroundStyle(.red)
            .transition(.opacity)
        }
      }
      .padding(.top, 24)

      Button(action: submit) {
        Text(isWorking ? workingAction : action)
          .frame(maxWidth: .infinity)
      }
      .buttonStyle(.glassProminent)
      .controlSize(.large)
      .keyboardShortcut(.defaultAction)
      .disabled(!canSubmit || isWorking)
      .padding(.top, 20)

      HStack(spacing: 16) { links }
        .buttonStyle(.link)
        .font(.callout)
        .padding(.top, 14)
    }
    .frame(width: 320)
    .padding(.horizontal, 48)
    .padding(.vertical, 40)
    .animation(.default, value: error)
  }
}

extension OnboardingCard where Links == EmptyView {
  init(
    title: LocalizedStringKey, message: Text? = nil, action: LocalizedStringKey,
    workingAction: LocalizedStringKey, isWorking: Bool, canSubmit: Bool, error: String?,
    submit: @escaping () -> Void, @ViewBuilder fields: () -> Fields
  ) {
    self.init(
      title: title, message: message, action: action, workingAction: workingAction,
      isWorking: isWorking, canSubmit: canSubmit, error: error, submit: submit, fields: fields,
      links: { EmptyView() })
  }
}
