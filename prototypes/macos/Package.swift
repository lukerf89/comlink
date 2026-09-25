// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "ComlinkPreview",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "ComlinkPreview", targets: ["ComlinkPreview"])],
    targets: [
        .target(name: "PrototypeCore"),
        .executableTarget(name: "ComlinkPreview", dependencies: ["PrototypeCore"]),
        .executableTarget(name: "PrototypeChecks", dependencies: ["PrototypeCore"], path: "Tests/PrototypeCoreTests")
    ]
)
