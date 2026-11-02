import SwiftUI

/// The line under a field: a hint, or what went wrong in its place.
struct FormFooter: View {
  let hint: LocalizedStringKey
  let error: String?

  var body: some View {
    Group {
      if let error {
        Text(verbatim: error)
          .foregroundStyle(Palette.destructive)
      } else {
        Text(hint)
          .foregroundStyle(Palette.mutedForeground)
      }
    }
    .font(.callout)
  }
}
