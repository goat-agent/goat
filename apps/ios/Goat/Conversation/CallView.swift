import AVKit
import SwiftUI

/// The screen of a call, kept as plain as a phone call: who, how long, and three buttons. A card
/// appears only when the agent needs the user to tap something, such as connecting a service.
struct CallView<Card: View>: View {
  let call: Call
  let agentName: String
  let isWorking: Bool
  @ViewBuilder let card: Card

  @Environment(\.dismiss) private var dismiss
  @Environment(\.openURL) private var openURL

  var body: some View {
    VStack(spacing: 0) {
      VStack(spacing: 8) {
        Text(agentName)
          .font(.largeTitle)
        status
          .font(.title3)
          .foregroundStyle(.secondary)
          .contentTransition(.opacity)
        if call.state == .failed(.microphoneDenied) {
          Button("Open Settings") {
            if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
          }
          .padding(.top, 8)
        }
      }
      .multilineTextAlignment(.center)
      .padding(.top, 96)
      .padding(.horizontal)

      Spacer()

      card
        .padding(.horizontal, 24)
        .padding(.bottom, 32)

      HStack(spacing: 36) {
        CallControl(title: "Audio") {
          AudioRoutePicker()
            .frame(width: 64, height: 64)
            .glassEffect(.regular.interactive(), in: .circle)
        }
        CallControl(title: call.isMuted ? "Unmute" : "Mute") {
          Button(action: call.toggleMute) {
            Image(systemName: call.isMuted ? "mic.slash.fill" : "mic.fill")
              .font(.title)
              .frame(width: 64, height: 64)
              .foregroundStyle(call.isMuted ? AnyShapeStyle(.background) : AnyShapeStyle(.primary))
              .background(
                call.isMuted ? AnyShapeStyle(.primary) : AnyShapeStyle(.clear), in: .circle)
          }
          .buttonStyle(.plain)
          .glassEffect(.regular.interactive(), in: .circle)
          .disabled(!isLive)
        }
        CallControl(title: "End") {
          Button(action: hangUp) {
            Image(systemName: "phone.down.fill")
              .font(.title)
              .frame(width: 64, height: 64)
          }
          .buttonStyle(.glassProminent)
          .buttonBorderShape(.circle)
          .tint(.red)
        }
      }
      .padding(.bottom, 48)
    }
    .frame(maxWidth: .infinity)
    .task { await call.start() }
    .onDisappear { call.end() }
    .onChange(of: call.state) {
      if call.state == .ended { dismiss() }
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
      } else if isWorking {
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
    dismiss()
  }
}

private struct CallControl<Control: View>: View {
  let title: LocalizedStringKey
  @ViewBuilder let control: Control

  var body: some View {
    VStack(spacing: 8) {
      control
      Text(title)
        .font(.caption)
        .foregroundStyle(.secondary)
    }
  }
}

/// The system's picker for where call audio plays: speaker, iPhone, AirPods, the car.
private struct AudioRoutePicker: UIViewRepresentable {
  func makeUIView(context: Context) -> AVRoutePickerView {
    let picker = AVRoutePickerView()
    picker.prioritizesVideoDevices = false
    picker.tintColor = .label
    picker.activeTintColor = .label
    return picker
  }

  func updateUIView(_ picker: AVRoutePickerView, context: Context) {}
}
