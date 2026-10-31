import GoatAPI
import SwiftUI

/// An agent in the list: who it is, what was said last, and whether it is working now.
struct AgentRow: View {
  let agent: Components.Schemas.Agent
  var avatarSize: CGFloat = 52

  private var isWorking: Bool { agent.status.value1 == .running }

  /// Mac's subheadline is 11 pt, too small to read under a name, so the Mac uses body there.
  private var detailFont: Font {
    #if os(macOS)
      .body
    #else
      .subheadline
    #endif
  }

  var body: some View {
    HStack(spacing: 12) {
      AvatarView(agent: agent, size: avatarSize)
      VStack(alignment: .leading, spacing: 1) {
        HStack(alignment: .firstTextBaseline) {
          Text(agent.name)
            .font(.headline)
            .lineLimit(1)
          Spacer(minLength: 8)
          if let date = agent.lastMessage?.createdAt {
            Text(LastActivity(date: date).text)
              .font(detailFont)
              .foregroundStyle(.secondary)
          }
        }
        // Two lines are always reserved, so every row is the same height whatever it shows.
        ZStack(alignment: .topLeading) {
          Text(preview)
            .lineLimit(2, reservesSpace: true)
            .opacity(isWorking ? 0 : 1)
          if isWorking {
            Image(systemName: "ellipsis")
              .font(.title3.weight(.semibold))
              .symbolEffect(.variableColor.iterative.dimInactiveLayers)
              .accessibilityLabel("Working")
          }
        }
        .font(detailFont)
        .foregroundStyle(.secondary)
        .frame(maxWidth: .infinity, alignment: .topLeading)
      }
    }
    .padding(.vertical, 2)
  }

  private var preview: String {
    guard let message = agent.lastMessage else { return "" }
    if !message.text.isEmpty { return message.text }
    guard let file = message.files?.first else { return "" }
    return file.mediaType.hasPrefix("image/") ? String(localized: "Photo") : file.name
  }
}

/// When something last happened, as briefly as Messages shows it.
private struct LastActivity {
  let date: Date

  var text: String {
    let calendar = Calendar.current
    if calendar.isDateInToday(date) {
      return date.formatted(date: .omitted, time: .shortened)
    }
    if calendar.isDateInYesterday(date) {
      return String(localized: "Yesterday")
    }
    if let days = calendar.dateComponents([.day], from: date, to: .now).day, days < 7 {
      return date.formatted(.dateTime.weekday(.wide))
    }
    return date.formatted(date: .numeric, time: .omitted)
  }
}
