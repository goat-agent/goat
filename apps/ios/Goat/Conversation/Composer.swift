import SwiftUI

/// Everything for writing a message sits in one shape: attachments, the text, dictation, and
/// one main button whose job follows the state (send, call, stop recording).
struct Composer: View {
  @Binding var draft: String
  let attachments: [ConversationModel.Attachment]
  let dictation: Dictation
  let canAttach: Bool
  let canSend: Bool
  let isEmpty: Bool
  let pickPhotos: () -> Void
  let pickFiles: () -> Void
  let open: (ConversationModel.Attachment) -> Void
  let retry: (ConversationModel.Attachment.ID) -> Void
  let remove: (ConversationModel.Attachment.ID) -> Void
  let send: () -> Void
  let call: () -> Void

  @Environment(\.openURL) private var openURL

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      VStack(alignment: .leading, spacing: 4) {
        if !attachments.isEmpty {
          attachmentList
        }
        HStack(alignment: .bottom, spacing: 2) {
          switch dictation.state {
          case .recording:
            Waveform(levels: dictation.levels)
              .padding(.leading, 12)
          case .transcribing:
            // The recording stays where it was, dimmed, until its text replaces it.
            Waveform(levels: dictation.levels)
              .opacity(0.3)
              .padding(.leading, 12)
              .accessibilityLabel("Transcribing…")
          case .idle:
            attachMenu
            TextField("Message", text: $draft, axis: .vertical)
              .lineLimit(1...5)
              .padding(.vertical, 8)
            ToolButton(title: "Dictate", systemImage: "mic") {
              Task { await dictation.start() }
            }
          }
          mainButton
        }
      }
      .padding(4)
      .glassEffect(.regular, in: .rect(cornerRadius: 22))

      if let problem = dictation.problem {
        problemFooter(problem)
      }
    }
    .padding(.horizontal)
    .padding(.bottom, 8)
    .sensoryFeedback(.impact(weight: .light), trigger: dictation.state) { old, new in
      new == .recording || old == .recording
    }
    .sensoryFeedback(.error, trigger: dictation.missCount)
    .animation(.default, value: dictation.state)
  }

  private var attachmentList: some View {
    ScrollView(.horizontal) {
      HStack(spacing: 10) {
        ForEach(attachments) { attachment in
          AttachmentChip(attachment: attachment) {
            open(attachment)
          } retry: {
            retry(attachment.id)
          } remove: {
            remove(attachment.id)
          }
        }
      }
      .padding([.top, .horizontal], 8)
    }
    .scrollIndicators(.hidden)
  }

  private var attachMenu: some View {
    Menu {
      Button("Photos", systemImage: "photo.on.rectangle", action: pickPhotos)
      Button("Files", systemImage: "doc", action: pickFiles)
    } label: {
      ToolButton.label(title: "Attach", systemImage: "plus")
    }
    .tint(.primary)
    .disabled(!canAttach)
  }

  @ViewBuilder private var mainButton: some View {
    switch dictation.state {
    case .recording:
      MainButton(title: "Stop Recording", systemImage: "stop.fill") {
        Task { await dictation.stop() }
      }
    case .transcribing:
      MainButton(title: "Transcribing…", systemImage: "", isWorking: true) {}
    case .idle where isEmpty:
      MainButton(title: "Call", systemImage: "phone.fill", action: call)
    case .idle:
      MainButton(title: "Send", systemImage: "arrow.up", isEnabled: canSend, action: send)
    }
  }

  @ViewBuilder private func problemFooter(_ problem: Dictation.Problem) -> some View {
    HStack {
      switch problem {
      case .microphoneDenied:
        FormFooter(hint: "", error: String(localized: "Allow microphone access in Settings."))
        Spacer()
        Button("Open Settings") {
          if let url = URL(string: UIApplication.openSettingsURLString) { openURL(url) }
        }
      case .transcriptionFailed:
        FormFooter(hint: "", error: String(localized: "Couldn’t transcribe your recording."))
        Spacer()
        Button("Try Again") {
          Task { await dictation.retry() }
        }
      }
    }
    .font(.footnote)
    .padding(.horizontal, 12)
  }
}

private struct ToolButton: View {
  let title: LocalizedStringKey
  let systemImage: String
  let action: () -> Void

  static func label(title: LocalizedStringKey, systemImage: String) -> some View {
    Label(title, systemImage: systemImage)
      .labelStyle(.iconOnly)
      .font(.title3)
      .foregroundStyle(.primary)
      .frame(width: 36, height: 36)
      .contentShape(.circle)
  }

  var body: some View {
    Button(action: action) {
      Self.label(title: title, systemImage: systemImage)
    }
    .buttonStyle(.plain)
  }
}

private struct MainButton: View {
  let title: LocalizedStringKey
  let systemImage: String
  var isWorking = false
  var isEnabled = true
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      ZStack {
        if isWorking {
          ProgressView()
            .tint(Color(.systemBackground))
        } else {
          Image(systemName: systemImage)
            .font(.body.weight(.semibold))
        }
      }
      .frame(width: 36, height: 36)
      // Fixed colors, because the glass around it would turn hierarchical styles translucent.
      .foregroundStyle(Color(.systemBackground))
      .background(Color(.label), in: .circle)
    }
    .buttonStyle(.plain)
    .opacity(isEnabled ? 1 : 0.3)
    .disabled(!isEnabled || isWorking)
    .accessibilityLabel(title)
  }
}

private struct Waveform: View {
  let levels: [Float]

  var body: some View {
    HStack(spacing: 3) {
      ForEach(Array(levels.enumerated()), id: \.offset) { _, level in
        Capsule()
          .frame(width: 3, height: 3 + CGFloat(level) * 24)
      }
    }
    .foregroundStyle(.primary)
    .frame(maxWidth: .infinity, minHeight: 36, alignment: .trailing)
    .clipped()
    .animation(.linear(duration: 0.05), value: levels)
    .accessibilityElement()
    .accessibilityLabel("Recording")
  }
}
