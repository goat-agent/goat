import SwiftUI

/// A section footer that shows the error in place of the hint, right under the field it belongs to.
struct FormFooter: View {
  let hint: LocalizedStringKey
  let error: String?

  var body: some View {
    if let error {
      Label(error, systemImage: "exclamationmark.circle.fill")
        .foregroundStyle(.red)
    } else {
      Text(hint)
    }
  }
}
