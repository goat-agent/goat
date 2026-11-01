import UIKit

final class AppDelegate: NSObject, UIApplicationDelegate {
  var model: AppModel?

  func application(
    _ application: UIApplication,
    didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data
  ) {
    Task { await model?.registerPushToken(deviceToken) }
  }
}
