import SwiftUI

struct MessageBubble: View {
  let text: String
  let isMine: Bool

  var body: some View {
    Group {
      if isMine {
        Text(text)
      } else {
        MarkdownText(text)
      }
    }
    .padding(.horizontal, 14)
    .padding(.vertical, 11)
    .foregroundStyle(isMine ? AnyShapeStyle(.white) : AnyShapeStyle(.primary))
    .background(
      isMine ? AnyShapeStyle(.tint) : AnyShapeStyle(.fill.secondary),
      in: .rect(cornerRadius: 22)
    )
    .contextMenu {
      Button("Copy", systemImage: "doc.on.doc") { Pasteboard.copy(text) }
    }
    .frame(maxWidth: .infinity, alignment: isMine ? .trailing : .leading)
    .padding(isMine ? .leading : .trailing, 64)
  }
}
