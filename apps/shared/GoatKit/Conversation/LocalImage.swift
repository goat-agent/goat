import ImageIO
import SwiftUI

/// An image file on this device, decoded at the size it is shown rather than at full size.
struct LocalImage: View {
  let url: URL?
  var contentMode: ContentMode = .fit
  var maxPixels: CGFloat = 800

  @State private var image: CGImage?

  var body: some View {
    Group {
      if let image {
        Image(decorative: image, scale: 1)
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
      guard let url else { return }
      image = await ImageScaling.thumbnail(at: url, maxPixels: maxPixels)
    }
  }
}
