import GoatAPI
import SwiftUI

/// One rounded field with files above it and the send button beside it. Return sends;
/// Option-Return starts a new line.
struct Composer: View {
  @Bindable var conversation: ConversationModel
  let pickFiles: () -> Void

  @FocusState private var isFocused: Bool

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      if !conversation.attachments.isEmpty {
        ScrollView(.horizontal) {
          HStack(spacing: 8) {
            ForEach(conversation.attachments) { attachment in
              AttachmentChip(attachment: attachment) {
                NSWorkspace.shared.open(attachment.localURL)
              } retry: {
                Task { await conversation.retryUpload(attachment.id) }
              } remove: {
                conversation.removeAttachment(attachment.id)
              }
            }
          }
        }
        .scrollIndicators(.never)
      }
      HStack(alignment: .bottom, spacing: 8) {
        HStack(alignment: .bottom, spacing: 6) {
          Button(action: pickFiles) {
            Image(systemName: "paperclip")
              .font(.system(size: 15))
              .foregroundStyle(.secondary)
              .frame(width: 28, height: 28)
              .contentShape(.rect)
          }
          .buttonStyle(.plain)
          .disabled(!conversation.canAttach)
          .help("Attach Files")
          .accessibilityLabel("Attach Files")

          TextField(
            "Message \(conversation.agent.name)", text: $conversation.draft, axis: .vertical
          )
          .textFieldStyle(.plain)
          .font(.body)
          .lineLimit(1...8)
          .focused($isFocused)
          .onSubmit(send)
          .padding(.vertical, 5)
        }
        .padding(.leading, 8)
        .padding(.trailing, 14)
        .padding(.vertical, 8)
        .frame(minHeight: 44)
        .glassEffect(.regular, in: .rect(cornerRadius: 22))

        Button(action: send) {
          Image(systemName: "arrow.up")
            .font(.system(size: 16, weight: .semibold))
            .foregroundStyle(.white)
            .frame(width: 44, height: 44)
            .background(.tint, in: .circle)
        }
        .buttonStyle(.plain)
        .disabled(!conversation.canSend)
        .opacity(conversation.canSend ? 1 : 0.35)
        .accessibilityLabel("Send")
      }
    }
    .onAppear { isFocused = true }
  }

  private func send() {
    guard conversation.canSend else { return }
    Task { await conversation.send() }
  }
}
