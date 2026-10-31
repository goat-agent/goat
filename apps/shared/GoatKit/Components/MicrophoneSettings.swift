import Foundation

#if os(iOS)
  import UIKit
#endif

/// Where people turn the microphone back on for goat after saying no.
enum MicrophoneSettings {
  static var url: URL? {
    #if os(iOS)
      URL(string: UIApplication.openSettingsURLString)
    #else
      URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
    #endif
  }
}
