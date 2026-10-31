import Foundation
import GoatAPI
import Observation

@Observable
final class SchedulesModel {
  private(set) var schedules: [Components.Schemas.Schedule] = []
  private(set) var hasLoaded = false

  private let agentID: String
  private let client: Client

  init(agentID: String, client: Client) {
    self.agentID = agentID
    self.client = client
  }

  func load() async {
    guard case .ok(let response) = try? await client.listSchedules(path: .init(id: agentID)),
      let list = try? response.body.json.schedules
    else { return }
    schedules = list
    hasLoaded = true
  }

  func setEnabled(_ isEnabled: Bool, for schedule: Components.Schemas.Schedule) async {
    let output = try? await client.updateSchedule(
      path: .init(id: agentID, scheduleId: schedule.id),
      body: .json(.init(enabled: isEnabled))
    )
    if case .ok = output { await load() }
  }

  func delete(_ schedule: Components.Schemas.Schedule) async {
    schedules.removeAll { $0.id == schedule.id }
    let output = try? await client.deleteSchedule(
      path: .init(id: agentID, scheduleId: schedule.id)
    )
    if case .noContent = output { return }
    await load()
  }
}
