// swift-tools-version: 6.2
import PackageDescription

let package = Package(
  name: "GoatAPI",
  platforms: [.iOS(.v26), .macOS(.v26)],
  products: [.library(name: "GoatAPI", targets: ["GoatAPI"])],
  dependencies: [
    .package(url: "https://github.com/apple/swift-openapi-generator", from: "1.14.3"),
    .package(url: "https://github.com/apple/swift-openapi-runtime", from: "1.13.3"),
    .package(url: "https://github.com/apple/swift-openapi-urlsession", from: "1.4.0"),
    .package(url: "https://github.com/apple/swift-http-types", from: "1.8.0"),
  ],
  targets: [
    .target(
      name: "GoatAPI",
      dependencies: [
        .product(name: "HTTPTypes", package: "swift-http-types"),
        .product(name: "OpenAPIRuntime", package: "swift-openapi-runtime"),
        .product(name: "OpenAPIURLSession", package: "swift-openapi-urlsession"),
      ],
      plugins: [.plugin(name: "OpenAPIGenerator", package: "swift-openapi-generator")]
    )
  ]
)
