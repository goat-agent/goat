import SwiftUI

struct OutgoingBubble: View {
  let message: ConversationModel.Outgoing
  let retry: () -> Void
  let discard: () -> Void

  var body: some View {
    VStack(alignment: .trailing, spacing: 4) {
      MessageBubble(text: message.text, isMine: true)
        .opacity(message.state == .failed ? 1 : 0.55)
      switch message.state {
      case .sending:
        Text("Sending…")
          .font(.caption2)
          .foregroundStyle(.secondary)
      case .queued:
        Text("Waiting")
          .font(.caption2)
          .foregroundStyle(.secondary)
      case .failed:
        Menu {
          Button("Send Again", systemImage: "arrow.clockwise", action: retry)
          Button("Delete", systemImage: "trash", role: .destructive, action: discard)
        } label: {
          Label("Not Sent", systemImage: "exclamationmark.circle.fill")
            .font(.caption)
            .foregroundStyle(.red)
        }
      }
    }
  }
}
