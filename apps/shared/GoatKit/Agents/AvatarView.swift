import GoatAPI
import SwiftUI

/// An agent's face wherever the agent appears: its picture, or the first letter of its name.
struct AvatarView: View {
  let agent: Components.Schemas.Agent
  let size: CGFloat

  @Environment(AppModel.self) private var model
  @State private var image: CGImage?

  var body: some View {
    AvatarCircle(name: agent.name, image: agent.avatarId == nil ? nil : image, size: size)
      .task(id: agent.avatarId) {
        guard let avatarID = agent.avatarId else { return }
        image = await AvatarStore.shared.image(
          agentID: agent.id, avatarID: avatarID, client: model.client)
      }
  }
}

/// The circle itself, for pictures that are not uploaded yet as well as stored ones.
struct AvatarCircle: View {
  let name: String
  let image: CGImage?
  let size: CGFloat

  var body: some View {
    Group {
      if let image {
        Image(decorative: image, scale: 1)
          .resizable()
          .scaledToFill()
      } else {
        Group {
          if let initial = name.first {
            Text(String(initial).uppercased())
              .font(.system(size: size * 0.42, weight: .semibold, design: .rounded))
          } else {
            // A new agent with no name yet: the circle is where its picture goes.
            Image(systemName: "camera.fill")
              .font(.system(size: size * 0.3))
          }
        }
        .foregroundStyle(.white)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(white: 0.56).gradient)
      }
    }
    .frame(width: size, height: size)
    .clipShape(.circle)
    .accessibilityHidden(true)
  }
}

/// Pictures by avatar id. An id never changes its bytes, so a picture is fetched at most once.
final class AvatarStore {
  static let shared = AvatarStore()

  private var images: [String: CGImage] = [:]

  func image(agentID: String, avatarID: String, client: Client) async -> CGImage? {
    if let image = images[avatarID] { return image }
    let name = "avatar.jpg"
    var url = FileCache.existing(id: avatarID, name: name)
    if url == nil, let data = try? await client.avatarData(agentID: agentID, avatarID: avatarID) {
      url = try? await FileCache.store(data, id: avatarID, name: name)
    }
    guard let url else { return nil }
    let image = await ImageScaling.thumbnail(at: url, maxPixels: CGFloat(AgentPhoto.side))
    images[avatarID] = image
    return image
  }
}
