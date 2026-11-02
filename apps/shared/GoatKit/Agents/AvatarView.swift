import GoatAPI
import SwiftUI

/// An agent's face wherever the agent appears: its photo, or the picture the server drew for it.
struct AvatarView: View {
  let agent: Components.Schemas.Agent
  let size: CGFloat

  @Environment(AppModel.self) private var model
  @State private var image: CGImage?

  var body: some View {
    Group {
      if let image {
        AvatarCircle(name: agent.name, image: image, size: size)
      } else {
        Circle()
          .fill(Palette.secondary)
          .frame(width: size, height: size)
      }
    }
    .task(id: agent.avatarId) {
      image = await AvatarStore.shared.image(
        agentID: agent.id, avatarID: agent.avatarId, client: model.client)
    }
  }
}

/// The circle itself, for pictures that are not uploaded yet as well as stored ones. Without a
/// picture it shows the first letter of a person's name, or where an agent's picture will go.
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
        if let initial = name.first {
          Text(String(initial).uppercased())
            .font(.system(size: size * 0.42, weight: .semibold, design: .rounded))
            .foregroundStyle(Palette.background)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Palette.foreground)
        } else {
          Image(systemName: "camera.fill")
            .font(.system(size: size * 0.3))
            .foregroundStyle(Palette.mutedForeground)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Palette.secondary)
        }
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
    let name = "avatar"
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
