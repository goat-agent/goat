#if os(iOS)
  import UIKit
  import UserNotifications

  /// Asks for permission to notify, registers with APNs, and decides how notifications show.
  final class Notifications: NSObject, UNUserNotificationCenterDelegate {
    /// The agent whose conversation is on screen, if any.
    var visibleAgentID: () -> String? = { nil }
    var open: (String) -> Void = { _ in }

    private var isRequesting = false

    override init() {
      super.init()
      UNUserNotificationCenter.current().delegate = self
    }

    /// Every launch registers again, because the token can change.
    func registerIfAuthorized() async {
      let settings = await UNUserNotificationCenter.current().notificationSettings()
      switch settings.authorizationStatus {
      case .authorized, .provisional, .ephemeral:
        UIApplication.shared.registerForRemoteNotifications()
      default:
        break
      }
    }

    /// Apple's guidance is to ask right after the user schedules their first task, not at launch.
    func requestIfNotDetermined() async {
      guard !isRequesting else { return }
      isRequesting = true
      defer { isRequesting = false }
      let center = UNUserNotificationCenter.current()
      guard await center.notificationSettings().authorizationStatus == .notDetermined else {
        return
      }
      let isGranted = (try? await center.requestAuthorization(options: [.alert, .sound])) ?? false
      if isGranted { UIApplication.shared.registerForRemoteNotifications() }
    }

    nonisolated func userNotificationCenter(
      _ center: UNUserNotificationCenter,
      willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
      let agentID = notification.request.content.userInfo["agentId"] as? String
      let isVisible = await MainActor.run { agentID != nil && agentID == visibleAgentID() }
      return isVisible ? [] : [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(
      _ center: UNUserNotificationCenter,
      didReceive response: UNNotificationResponse
    ) async {
      guard let agentID = response.notification.request.content.userInfo["agentId"] as? String
      else { return }
      await MainActor.run { open(agentID) }
    }
  }
#else
  /// The Mac does not notify yet; this keeps `AppModel` the same on both devices.
  final class Notifications {
    var visibleAgentID: () -> String? = { nil }
    var open: (String) -> Void = { _ in }

    func registerIfAuthorized() async {}
    func requestIfNotDetermined() async {}
  }
#endif
