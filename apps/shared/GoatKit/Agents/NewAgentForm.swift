import SwiftUI

/// The agent form for an agent that does not exist yet; the picture waits on this device until
/// the agent is created, and the server draws one when there is none.
struct NewAgentForm: View {
  @Binding var name: String
  @Binding var instructions: String
  @Binding var photo: Data?

  @State private var preview: CGImage?

  var body: some View {
    AgentForm(
      name: $name,
      instructions: $instructions,
      hasPhoto: photo != nil,
      pickPhoto: { photo = $0 },
      removePhoto: { photo = nil }
    ) {
      if let preview {
        AvatarCircle(name: name, image: preview, size: 64)
      } else {
        Circle()
          .fill(Palette.secondary)
          .frame(width: 64, height: 64)
      }
    }
    .task(id: photo) {
      preview = photo.flatMap { ImageScaling.thumbnail(of: $0, longEdge: CGFloat(AgentPhoto.side)) }
    }
  }
}
