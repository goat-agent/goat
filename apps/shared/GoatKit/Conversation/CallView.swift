import AuthenticationServices
import GoatAPI
import SwiftUI

/// The screen of a call, kept as plain as a phone call: who, how long, and three buttons. A card
/// appears only when the agent needs the user to tap something, such as connecting a service.
struct CallView: View {
  let call: Call
  let agent: Components.Schemas.Agent
  let conversation: ConversationModel
  let integrations: IntegrationsModel
  /// Takes the call screen away: a full-screen cover on iPhone, the conversation's place on Mac.
  let close: () -> Void

  @Environment(\.openURL) private var openURL
  @Environment(\.webAuthenticationSession) private var webAuthenticationSession

  var body: some View {
    VStack(spacing: 0) {
      VStack(spacing: 4) {
        AvatarView(agent: agent, size: 128)
          .padding(.bottom, 16)
        Text(agent.name)
          .font(.largeTitle.bold())
        status
          .font(.title3)
          .foregroundStyle(.secondary)
          .contentTransition(.opacity)
        if call.state == .failed(.microphoneDenied) {
          Button("Open Settings") {
            if let url = MicrophoneSettings.url { openURL(url) }
          }
          .padding(.top, 8)
        }
      }
      .multilineTextAlignment(.center)
      .padding(.top, 120)
      .padding(.horizontal)

      Spacer()

      if let request = conversation.pendingConnect(in: integrations) {
        ConnectCard(
          request: request, integrations: integrations, agentID: agent.id,
          session: webAuthenticationSession
        )
        .fixedSize()
        .padding(.horizontal, 24)
        .padding(.bottom, 32)
      }

      HStack(spacing: 32) {
        // A Mac plays calls wherever its sound output is set, so only iPhone has a speaker button.
        #if os(iOS)
          ToggleControl(
            title: "Speaker", systemImage: "speaker.wave.2.fill", isOn: call.isOnSpeaker,
            action: call.toggleSpeaker
          )
          .disabled(!isLive)
        #endif
        ToggleControl(
          title: "Mute", systemImage: "mic.slash.fill", isOn: call.isMuted,
          action: call.toggleMute
        )
        .disabled(!isLive)
        Button(action: hangUp) {
          Image(systemName: "phone.down.fill")
            .font(.title)
            .frame(width: 68, height: 68)
        }
        .buttonStyle(.glassProminent)
        .buttonBorderShape(.circle)
        .tint(.red)
        .accessibilityLabel("End")
      }
      .padding(.bottom, 56)
    }
    .frame(maxWidth: .infinity)
    .task { await call.start() }
    .onDisappear { call.end() }
    .onChange(of: call.state) {
      if call.state == .ended { close() }
    }
    .sensoryFeedback(.impact(weight: .light), trigger: call.isMuted)
  }

  private var isLive: Bool {
    if case .connected = call.state { true } else { false }
  }

  @ViewBuilder private var status: some View {
    switch call.state {
    case .connecting:
      Text("Calling…")
    case .connected(let since):
      if call.isMuted {
        Text("Muted")
      } else if conversation.isRunning {
        Text("Working on it…")
      } else {
        Text(since, style: .timer)
      }
    case .failed(.microphoneDenied):
      Text("Allow microphone access in Settings.")
    case .failed(.unreachable):
      Text("Couldn’t connect the call.")
    case .ended:
      Text("Call Ended")
    }
  }

  private func hangUp() {
    call.end()
    close()
  }
}

/// A call setting that is on or off. On fills the circle, as in the Phone app.
private struct ToggleControl: View {
  let title: LocalizedStringKey
  let systemImage: String
  let isOn: Bool
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      Image(systemName: systemImage)
        .font(.title)
        .frame(width: 68, height: 68)
        .foregroundStyle(isOn ? AnyShapeStyle(.background) : AnyShapeStyle(.primary))
        .background(isOn ? AnyShapeStyle(.primary) : AnyShapeStyle(.clear), in: .circle)
    }
    .buttonStyle(.plain)
    // Glass over the filled circle would turn it translucent, so an on control is drawn solid.
    .glassEffect(isOn ? .identity : .regular.interactive(), in: .circle)
    .accessibilityLabel(title)
    .accessibilityAddTraits(isOn ? .isSelected : [])
  }
}
