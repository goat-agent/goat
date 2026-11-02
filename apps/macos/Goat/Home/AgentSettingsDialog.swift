import GoatAPI
import SwiftUI

/// One agent's settings over the window, laid out like the Settings window: the agent and its
/// pages on the left, the open page on the right. A new kind of setting is a new page.
struct AgentSettingsDialog: View {
  let model: AppModel
  let agent: Components.Schemas.Agent
  let close: () -> Void

  @State private var page = Page.general
  @State private var schedules: SchedulesModel

  init(model: AppModel, agent: Components.Schemas.Agent, close: @escaping () -> Void) {
    self.model = model
    self.agent = agent
    self.close = close
    _schedules = State(initialValue: SchedulesModel(agentID: agent.id, client: model.client))
  }

  private enum Page: CaseIterable, Identifiable {
    case general
    case schedules

    var id: Self { self }

    var title: LocalizedStringKey {
      switch self {
      case .general: "General"
      case .schedules: "Schedules"
      }
    }

    var systemImage: String {
      switch self {
      case .general: "person.crop.circle"
      case .schedules: "clock"
      }
    }
  }

  private static let shape = RoundedRectangle(cornerRadius: 20, style: .continuous)

  var body: some View {
    HStack(spacing: 0) {
      VStack(alignment: .leading, spacing: 2) {
        HStack(spacing: 10) {
          AvatarView(agent: agent, size: 28)
          Text(agent.name)
            .font(.body.weight(.semibold))
            .foregroundStyle(Palette.foreground)
            .lineLimit(1)
        }
        .padding(.horizontal, 10)
        .padding(.bottom, 14)
        ForEach(Page.allCases) { item in
          PageRow(title: item.title, systemImage: item.systemImage, isSelected: item == page) {
            page = item
          }
        }
      }
      .padding(.top, 20)
      .padding(.horizontal, 8)
      .frame(width: 184, alignment: .topLeading)
      .frame(maxHeight: .infinity, alignment: .top)

      ScrollView {
        VStack(alignment: .leading, spacing: 14) {
          Text(page.title)
            .font(.sectionTitle)
            .foregroundStyle(Palette.foreground)
          switch page {
          case .general:
            AgentSettingsForm(model: model, agent: agent, onDelete: close)
          case .schedules:
            SchedulesSection(model: schedules)
          }
        }
        .padding(.top, 20)
        .padding(.leading, 12)
        .padding(.trailing, 52)
        .padding(.bottom, 20)
        .frame(maxWidth: .infinity, alignment: .leading)
      }
      .scrollIndicators(.never)
    }
    .frame(maxWidth: 640, maxHeight: 460)
    .overlay(alignment: .topTrailing) {
      Button("Close", systemImage: "xmark", action: close)
        .buttonStyle(.icon)
        .keyboardShortcut(.cancelAction)
        .padding(12)
    }
    .background(Palette.popover, in: Self.shape)
    // The photo menu and the delete confirmation open inside the dialog.
    .portalHost()
    .clipShape(Self.shape)
    .overlay { Self.shape.strokeBorder(Palette.border, lineWidth: 0.5) }
    .shadow(color: .black.opacity(0.2), radius: 24, y: 12)
  }
}
