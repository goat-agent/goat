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
          .clipShape(.rect(cornerRadius: 14, style: .continuous))
      } else {
        HStack(spacing: 10) {
          Group {
            if isOpening { ProgressView().controlSize(.small) } else { Image(systemName: "doc") }
          }
          .frame(width: 16)
          Text(file.name)
            .lineLimit(1)
            .truncationMode(.middle)
        }
        .foregroundStyle(Palette.foreground)
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(Palette.secondary, in: .rect(cornerRadius: 14, style: .continuous))
      }
    }
    .buttonStyle(.plain)
    .accessibilityLabel(isImage ? Text("Photo") : Text(file.name))
    .frame(maxWidth: .infinity, alignment: .trailing)
    .padding(.leading, 64)
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
