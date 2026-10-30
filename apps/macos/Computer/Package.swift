// swift-tools-version: 6.2
import PackageDescription

let package = Package(
  name: "Computer",
  platforms: [.macOS(.v26)],
  products: [.library(name: "Computer", targets: ["Computer"])],
  dependencies: [.package(path: "../../shared/GoatAPI")],
  targets: [
    .target(name: "Computer", dependencies: [.product(name: "GoatAPI", package: "GoatAPI")]),
    .testTarget(name: "ComputerTests", dependencies: ["Computer"]),
  ]
)
