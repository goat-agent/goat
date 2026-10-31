import SwiftUI

/// Everything for writing a message sits in one shape: attachments, the text, dictation, and
/// one main button whose job follows the state (send, call, stop recording).
struct Composer: View {
  @Bindable var conversation: ConversationModel
  let pickPhotos: () -> Void
  let pickFiles: () -> Void
  let open: (URL) -> Void
  let call: () -> Void

  @Environment(\.openURL) private var openURL

  private var dictation: Dictation { conversation.dictation }

  var body: some View {
    VStack(alignment: .leading, spacing: 4) {
      if let problem = dictation.problem {
        problemRow(problem)
        Divider()
      }
      if !conversation.attachments.isEmpty {
        attachmentList
      }
      HStack(alignment: .bottom, spacing: 2) {
        switch dictation.state {
        case .recording:
          ToolButton(title: "Discard Recording", systemImage: "xmark") { dictation.cancel() }
          Waveform(levels: dictation.levels)
        case .transcribing:
          // The recording stays where it was, dimmed, until its text replaces it.
          ToolButton(title: "Discard Recording", systemImage: "xmark") {}
            .disabled(true)
          Waveform(levels: dictation.levels)
            .opacity(0.3)
            .accessibilityLabel("Transcribing…")
        case .idle:
          attachMenu
          TextField("Message", text: $conversation.draft, axis: .vertical)
            .textFieldStyle(.plain)
            .lineLimit(1...5)
            #if os(macOS)
              // Return sends on the Mac, as in Messages; Option-Return starts a new line.
              .onSubmit(send)
            #endif
            .padding(.vertical, 8)
          ToolButton(title: "Dictate", systemImage: "mic") {
            Task { await dictation.start() }
          }
          .foregroundStyle(.secondary)
        }
        mainButton
      }
    }
    .padding(4)
    .glassEffect(.regular, in: .rect(cornerRadius: 22))
    .sensoryFeedback(.impact(weight: .light), trigger: dictation.state) { old, new in
      new == .recording || old == .recording
    }
    .sensoryFeedback(.error, trigger: dictation.missCount)
    .animation(.default, value: dictation.state)
  }

  private var attachmentList: some View {
    ScrollView(.horizontal) {
      HStack(spacing: 10) {
        ForEach(conversation.attachments) { attachment in
          AttachmentChip(attachment: attachment) {
            open(attachment.localURL)
          } retry: {
            Task { await conversation.retryUpload(attachment.id) }
          } remove: {
            conversation.removeAttachment(attachment.id)
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
    .menuStyle(.button)
    .buttonStyle(.plain)
    .menuIndicator(.hidden)
    .tint(.primary)
    .disabled(!conversation.canAttach)
  }

  @ViewBuilder private var mainButton: some View {
    switch dictation.state {
    case .recording:
      MainButton(title: "Stop Recording", systemImage: "stop.fill") {
        Task { await dictation.stop() }
      }
    case .transcribing:
      MainButton(title: "Transcribing…", systemImage: "", isWorking: true) {}
    case .idle where conversation.isEmpty:
      MainButton(title: "Call", systemImage: "waveform", action: call)
    case .idle:
      MainButton(
        title: "Send", systemImage: "arrow.up", isEnabled: conversation.canSend, action: send)
    }
  }

  private func send() {
    guard conversation.canSend else { return }
    Task { await conversation.send() }
  }

  /// A problem with writing stays inside the composer, next to what it is about.
  @ViewBuilder private func problemRow(_ problem: Dictation.Problem) -> some View {
    HStack {
      switch problem {
      case .microphoneDenied:
        Text("Microphone is off.")
          .foregroundStyle(.red)
        Spacer()
        Button("Settings") {
          if let url = MicrophoneSettings.url { openURL(url) }
        }
      case .transcriptionFailed:
        Text("Couldn’t transcribe.")
          .foregroundStyle(.red)
        Spacer()
        Button("Try Again") {
          Task { await dictation.retry() }
        }
      }
    }
    .font(.subheadline)
    .fontWeight(.medium)
    .tint(.primary)
    .padding(.horizontal, 12)
    .frame(minHeight: 36)
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
            .tint(.white)
        } else {
          Image(systemName: systemImage)
            .font(.body.weight(.semibold))
        }
      }
      .frame(width: 36, height: 36)
      // Fixed colors, because the glass around it would turn hierarchical styles translucent.
      .foregroundStyle(.white)
      .background(Color.accentColor, in: .circle)
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
