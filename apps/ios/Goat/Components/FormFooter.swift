import SwiftUI

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
