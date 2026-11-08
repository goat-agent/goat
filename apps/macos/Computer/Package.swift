// swift-tools-version: 6.2
import PackageDescription

let settings: [SwiftSetting] = [.enableUpcomingFeature("NonisolatedNonsendingByDefault")]

let package = Package(
  name: "Computer",
  platforms: [.macOS(.v26)],
  products: [.library(name: "Computer", targets: ["Computer"])],
  targets: [
    .target(name: "Computer", swiftSettings: settings),
    .testTarget(name: "ComputerTests", dependencies: ["Computer"], swiftSettings: settings),
  ]
)
