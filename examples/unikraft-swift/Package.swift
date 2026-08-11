// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "server",
    targets: [
        .executableTarget(name: "server", path: "Sources/server")
    ]
)
