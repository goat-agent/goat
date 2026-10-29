import SwiftUI

struct SignInFlow: View {
  @Bindable var model: AppModel

  var body: some View {
    NavigationStack(path: $model.signInPath) {
      EmailView(model: model)
        .navigationDestination(for: String.self) { email in
          CodeView(model: model, email: email)
        }
    }
  }
}
