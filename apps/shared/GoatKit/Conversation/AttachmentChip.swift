import SwiftUI
import UniformTypeIdentifiers

/// A file waiting in the composer, with its upload state.
struct AttachmentChip: View {
  let attachment: ConversationModel.Attachment
  let open: () -> Void
  let retry: () -> Void
  let remove: () -> Void

  private var isImage: Bool {
    UTType(filenameExtension: attachment.localURL.pathExtension)?.conforms(to: .image) == true
  }

  private var didFail: Bool {
    if case .failed = attachment.state { true } else { false }
  }

  var body: some View {
    Button(action: didFail ? retry : open) {
      ZStack {
        if isImage {
          LocalImage(url: attachment.localURL, contentMode: .fill, maxPixels: 200)
        } else {
          VStack(spacing: 4) {
            Image(systemName: "doc.text")
              .font(.title3)
            Text(attachment.name)
              .font(.caption2)
              .lineLimit(2)
              .truncationMode(.middle)
          }
          .padding(6)
          .frame(maxWidth: .infinity, maxHeight: .infinity)
          .background(.fill.secondary)
        }
        switch attachment.state {
        case .uploading:
          Rectangle().fill(.black.opacity(0.3))
          ProgressView().tint(.white)
        case .failed:
          Rectangle().fill(.black.opacity(0.45))
          Image(systemName: "arrow.clockwise")
            .font(.headline)
            .foregroundStyle(.white)
        case .uploaded:
          EmptyView()
        }
      }
      .frame(width: 64, height: 64)
      .clipShape(.rect(cornerRadius: 12))
    }
    .buttonStyle(.plain)
    .accessibilityLabel(
      didFail ? Text("\(attachment.name), Not Uploaded. Upload Again") : Text(attachment.name)
    )
    .overlay(alignment: .topTrailing) {
      Button("Remove", systemImage: "xmark.circle.fill", action: remove)
        .labelStyle(.iconOnly)
        .symbolRenderingMode(.palette)
        .foregroundStyle(.white, .black.opacity(0.6))
        .font(.title3)
        .offset(x: 6, y: -6)
    }
  }
}
