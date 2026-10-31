import GoatAPI
import SwiftUI

/// What the agent has scheduled. Schedules are made in the conversation, so there is no add button.
struct SchedulesSection: View {
  let model: SchedulesModel

  var body: some View {
    Section("Schedules") {
      if model.hasLoaded && model.schedules.isEmpty {
        Text("Ask in the chat, like “Remind me every day at 8,” and it shows up here.")
          .foregroundStyle(.secondary)
      }
      ForEach(model.schedules, id: \.id) { schedule in
        ScheduleRow(schedule: schedule)
          .swipeActions {
            Button("Delete", role: .destructive) {
              Task { await model.delete(schedule) }
            }
          }
          .contextMenu {
            Button(
              schedule.enabled ? "Pause" : "Resume",
              systemImage: schedule.enabled ? "pause" : "play"
            ) {
              Task { await model.setEnabled(!schedule.enabled, for: schedule) }
            }
            Button("Delete", systemImage: "trash", role: .destructive) {
              Task { await model.delete(schedule) }
            }
          }
      }
    }
  }
}

private struct ScheduleRow: View {
  let schedule: Components.Schemas.Schedule

  var body: some View {
    HStack(spacing: 12) {
      VStack(alignment: .leading, spacing: 2) {
        Text(schedule.title)
        detail
          .font(.subheadline)
          .foregroundStyle(.secondary)
      }
      Spacer(minLength: 8)
      if schedule.repeats {
        Image(systemName: "repeat")
          .foregroundStyle(.secondary)
          .accessibilityLabel("Repeats")
      }
    }
    .opacity(schedule.enabled ? 1 : 0.5)
  }

  @ViewBuilder private var detail: some View {
    if !schedule.enabled {
      Text("Paused")
    } else {
      HStack(spacing: 4) {
        switch schedule.problem?.value1 {
        case .missed: Text("Missed")
        case .failed: Text("Didn’t run")
        case nil: EmptyView()
        }
        if schedule.problem?.value1 != nil && schedule.nextRunAt != nil { Text("·") }
        if let date = schedule.nextRunAt {
          Text(date, format: .relative(presentation: .named, unitsStyle: .abbreviated))
        }
      }
    }
  }
}
