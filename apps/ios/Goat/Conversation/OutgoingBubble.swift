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
        Text("보내는 중…")
          .font(.caption2)
          .foregroundStyle(.secondary)
      case .queued:
        Text("대기 중")
          .font(.caption2)
          .foregroundStyle(.secondary)
      case .failed:
        Menu {
          Button("다시 보내기", systemImage: "arrow.clockwise", action: retry)
          Button("삭제", systemImage: "trash", role: .destructive, action: discard)
        } label: {
          Label("보내지 못했어요", systemImage: "exclamationmark.circle.fill")
            .font(.caption)
            .foregroundStyle(.red)
        }
      }
    }
  }
}
