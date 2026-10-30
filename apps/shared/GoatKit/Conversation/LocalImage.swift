import SwiftUI
import UIKit

/// An image file on this device, decoded at the size it is shown rather than at full size.
struct LocalImage: View {
  let url: URL?
  var contentMode: ContentMode = .fit
  var maxPixels: CGFloat = 800

  @State private var image: UIImage?

  var body: some View {
    Group {
      if let image {
        Image(uiImage: image)
          .resizable()
          .aspectRatio(contentMode: contentMode)
      } else {
        Rectangle()
          .fill(.fill.secondary)
          .aspectRatio(4 / 3, contentMode: contentMode)
          .overlay { ProgressView() }
      }
    }
    .task(id: url) {
      guard let url, let full = UIImage(contentsOfFile: url.path(percentEncoded: false)) else {
        return
      }
      let scale = min(1, maxPixels / max(full.size.width, full.size.height))
      let size = CGSize(width: full.size.width * scale, height: full.size.height * scale)
      image = await full.byPreparingThumbnail(ofSize: size) ?? full
    }
  }
}
