import GoatAPI
import SwiftUI

struct OutgoingBubble: View {
  let message: ConversationModel.Outgoing
  let load: (Components.Schemas.File) async -> URL?
  let open: (URL) -> Void
  let retry: () -> Void
  let discard: () -> Void

  var body: some View {
    VStack(alignment: .trailing, spacing: 4) {
      Group {
        ForEach(message.files, id: \.id) { file in
          SentFile(file: file, load: load, open: open)
        }
        if !message.text.isEmpty {
          MessageBubble(text: message.text, isMine: true)
        }
      }
      .opacity(message.state == .failed ? 1 : 0.55)
      Group {
        switch message.state {
        case .sending:
          Text("Sending…")
            .foregroundStyle(Palette.mutedForeground)
        case .queued:
          Text("Waiting")
            .foregroundStyle(Palette.mutedForeground)
        case .failed:
          HStack(spacing: 2) {
            Text("Not Sent")
              .foregroundStyle(Palette.destructive)
            Button("Send Again", action: retry)
            Button("Delete", role: .destructive, action: discard)
          }
          .buttonStyle(.ghost)
          .controlSize(.small)
        }
      }
      .font(.caption)
    }
  }
}
