import SwiftUI

struct IntegrationIcon: View {
  let url: String?
  var size: CGFloat = 30

  var body: some View {
    AsyncImage(url: url.flatMap(URL.init(string:))) { image in
      image.resizable().scaledToFit()
    } placeholder: {
      Color.white
    }
    .frame(width: size, height: size)
    .clipShape(.rect(cornerRadius: size * 0.24))
    .overlay {
      RoundedRectangle(cornerRadius: size * 0.24)
        .strokeBorder(.separator, lineWidth: 0.5)
    }
    .accessibilityHidden(true)
  }
}
