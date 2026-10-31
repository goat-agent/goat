import LinkPresentation
import SwiftUI

/// A link sent on its own, shown as the system's link preview the way Messages shows one.
struct LinkCard: View {
  let url: URL
  @State private var metadata: LPLinkMetadata?

  /// The link when a bubble holds nothing but one web address.
  static func url(in text: String) -> URL? {
    guard !text.contains(where: \.isWhitespace), let url = URL(string: text),
      url.scheme == "https" || url.scheme == "http", url.host() != nil
    else { return nil }
    return url
  }

  var body: some View {
    LinkPreview(url: url, metadata: metadata)
      .frame(maxWidth: .infinity, alignment: .leading)
      .padding(.trailing, 48)
      .task(id: url) { metadata = await LinkMetadataCache.shared.metadata(for: url) }
  }
}

private let maxPreviewWidth: CGFloat = 280

#if os(iOS)
  private struct LinkPreview: UIViewRepresentable {
    let url: URL
    let metadata: LPLinkMetadata?

    func makeUIView(context: Context) -> LPLinkView {
      LPLinkView(url: url)
    }

    func updateUIView(_ view: LPLinkView, context: Context) {
      if let metadata { view.metadata = metadata }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView: LPLinkView, context: Context)
      -> CGSize?
    {
      let width = min(proposal.width ?? maxPreviewWidth, maxPreviewWidth)
      return uiView.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude))
    }
  }
#else
  private struct LinkPreview: NSViewRepresentable {
    let url: URL
    let metadata: LPLinkMetadata?

    func makeNSView(context: Context) -> LPLinkView {
      LPLinkView(url: url)
    }

    func updateNSView(_ view: LPLinkView, context: Context) {
      if let metadata { view.metadata = metadata }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: LPLinkView, context: Context)
      -> CGSize?
    {
      let width = min(proposal.width ?? maxPreviewWidth, maxPreviewWidth)
      return CGSize(width: width, height: nsView.fittingSize.height)
    }
  }
#endif

/// Fetched previews, kept so scrolling back does not load the page again.
private final class LinkMetadataCache {
  static let shared = LinkMetadataCache()
  private var fetched: [URL: LPLinkMetadata] = [:]

  func metadata(for url: URL) async -> LPLinkMetadata? {
    if let metadata = fetched[url] { return metadata }
    guard let metadata = try? await LPMetadataProvider().startFetchingMetadata(for: url) else {
      return nil
    }
    fetched[url] = metadata
    return metadata
  }
}
