import GoatAPI
import SwiftUI

/// A file in one of the user's messages. Images show inline; other files show as a card and
/// download only when opened.
struct SentFile: View {
  let file: Components.Schemas.File
  let load: (Components.Schemas.File) async -> URL?
  let open: (URL) -> Void

  @State private var url: URL?
  @State private var isOpening = false

  private var isImage: Bool { file.mediaType.hasPrefix("image/") }

  var body: some View {
    Button(action: tap) {
      if isImage {
        LocalImage(url: url)
          .frame(maxWidth: 220, maxHeight: 280)
          .clipShape(.rect(cornerRadius: 18))
      } else {
        Label {
          Text(file.name)
            .lineLimit(1)
            .truncationMode(.middle)
        } icon: {
          if isOpening { ProgressView().tint(.white) } else { Image(systemName: "doc.text") }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .foregroundStyle(.white)
        .background(.tint, in: .rect(cornerRadius: 20))
      }
    }
    .buttonStyle(.plain)
    .accessibilityLabel(isImage ? "사진" : file.name)
    .frame(maxWidth: .infinity, alignment: .trailing)
    .padding(.leading, 48)
    .task(id: file.id) {
      if isImage { url = await load(file) }
    }
  }

  private func tap() {
    if let url { return open(url) }
    guard !isOpening else { return }
    isOpening = true
    Task {
      url = await load(file)
      isOpening = false
      if let url { open(url) }
    }
  }
}
