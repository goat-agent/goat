import SwiftUI

struct MessageBubble: View {
  let text: String
  let isMine: Bool

  var body: some View {
    Bubble(variant: isMine ? .primary : .secondary) {
      if isMine {
        Text(text)
      } else {
        MarkdownText(text)
      }
    }
    .contextMenu {
      Button("Copy", systemImage: "doc.on.doc") { Pasteboard.copy(text) }
    }
    .frame(maxWidth: .infinity, alignment: isMine ? .trailing : .leading)
    .padding(isMine ? .leading : .trailing, 64)
  }
}
