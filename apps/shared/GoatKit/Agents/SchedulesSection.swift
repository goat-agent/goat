import GoatAPI
import SwiftUI

/// What the agent has scheduled. Schedules are made in the conversation, so there is no add button.
struct SchedulesSection: View {
  let model: SchedulesModel

  var body: some View {
    Group {
      if !model.hasLoaded {
        ProgressView()
          .controlSize(.small)
      } else if model.schedules.isEmpty {
        Text("Ask in the chat, like “Remind me every day at 8,” and it shows up here.")
          .foregroundStyle(Palette.mutedForeground)
          .fixedSize(horizontal: false, vertical: true)
      } else {
        ItemGroup {
          ForEach(model.schedules, id: \.id) { schedule in
            ScheduleRow(schedule: schedule) {
              Task { await model.setEnabled(!schedule.enabled, for: schedule) }
            } delete: {
              Task { await model.delete(schedule) }
            }
          }
        }
      }
    }
    .task { await model.load() }
  }
}

private struct ScheduleRow: View {
  let schedule: Components.Schemas.Schedule
  let toggle: () -> Void
  let delete: () -> Void

  @State private var isShowingMenu = false

  var body: some View {
    Item(title: schedule.title, description: detail) {
      if schedule.repeats {
        IconSlot(systemName: "repeat")
          .foregroundStyle(Palette.mutedForeground)
          .accessibilityElement()
          .accessibilityLabel("Repeats")
      }
      Button("More", systemImage: "ellipsis") { isShowingMenu = true }
        .buttonStyle(.icon)
        .dropdownMenu(isPresented: $isShowingMenu, placement: .below(.trailing)) {
          DropdownMenuItem(
            title: schedule.enabled ? "Pause" : "Resume",
            systemImage: schedule.enabled ? "pause" : "play", action: toggle)
          DropdownMenuItem(
            title: "Delete", systemImage: "trash", role: .destructive, action: delete)
        }
    }
    .opacity(schedule.enabled ? 1 : 0.5)
  }

  private var detail: String? {
    guard schedule.enabled else { return String(localized: "Paused") }
    let problem: String? =
      switch schedule.problem?.value1 {
      case .missed: String(localized: "Missed")
      case .failed: String(localized: "Didn’t run")
      case nil: nil
      }
    let next = schedule.nextRunAt?.formatted(
      .relative(presentation: .named, unitsStyle: .abbreviated))
    let parts = [problem, next].compactMap(\.self)
    return parts.isEmpty ? nil : parts.joined(separator: " · ")
  }
}
