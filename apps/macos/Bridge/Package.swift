// swift-tools-version: 6.2
import PackageDescription

let package = Package(
  name: "Bridge",
  platforms: [.macOS(.v26)],
  products: [.library(name: "Bridge", targets: ["Bridge"])],
  dependencies: [
    .package(url: "https://github.com/apple/swift-openapi-generator", from: "1.14.3"),
    .package(url: "https://github.com/apple/swift-openapi-runtime", from: "1.13.3"),
  ],
  targets: [
    .target(
      name: "Bridge",
      dependencies: [.product(name: "OpenAPIRuntime", package: "swift-openapi-runtime")],
      plugins: [.plugin(name: "OpenAPIGenerator", package: "swift-openapi-generator")]
    )
  ]
)
