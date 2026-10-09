// swift-tools-version: 6.3
import Foundation
import PackageDescription

#if os(Linux)
  let linksDebugCdylib = true
#else
  let linksDebugCdylib =
    ProcessInfo.processInfo.environment["BRIDGETHING_COMPANION_CDYLIB"] == "1"
#endif

let debugLibraryDir = "\(Context.packageDirectory)/target/debug"

let companionCoreTargets: [Target] =
  linksDebugCdylib
  ? [
    .systemLibrary(
      name: "bridgething_companionFFI",
      path: "packages/companion/swift/FFI/bridgething_companionFFI"
    ),
    .target(
      name: "BridgethingCompanionCore",
      dependencies: ["bridgething_companionFFI"],
      path: "packages/companion/swift/Sources/BridgethingCompanionCore",
      linkerSettings: [
        .unsafeFlags([
          "-L\(debugLibraryDir)", "-Xlinker", "-rpath", "-Xlinker", debugLibraryDir,
        ]),
        .linkedLibrary("bridgething_companion"),
      ]
    ),
  ]
  : [
    .binaryTarget(
      name: "BridgethingCompanionCoreFFI",
      path: "packages/companion/swift/Frameworks/BridgethingCompanionCoreFFI.xcframework"
    ),
    .target(
      name: "BridgethingCompanionCore",
      dependencies: ["BridgethingCompanionCoreFFI"],
      path: "packages/companion/swift/Sources/BridgethingCompanionCore"
    ),
  ]

let package = Package(
  name: "Bridgething",
  platforms: [
    .iOS(.v18),
    .macOS(.v15),
  ],
  products: [
    .library(name: "BridgethingCompanion", targets: ["BridgethingCompanion"]),
    .library(name: "BridgethingCompanionCore", targets: ["BridgethingCompanionCore"]),
  ],
  dependencies: [
    .package(url: "https://github.com/apple/swift-log", from: "1.6.0")
  ],
  targets: [
    .target(
      name: "BridgethingCompanion",
      dependencies: [
        "BridgethingCompanionCore",
        "BridgethingCrashHooks",
        .product(name: "Logging", package: "swift-log"),
      ],
      path: "packages/companion/swift/Sources/BridgethingCompanion",
      resources: [.process("Resources")]
    ),
    .target(
      name: "BridgethingCrashHooks",
      path: "packages/companion/swift/Sources/BridgethingCrashHooks"
    ),
    .testTarget(
      name: "BridgethingCompanionCoreTests",
      dependencies: ["BridgethingCompanionCore"],
      path: "packages/companion/swift/Tests/BridgethingCompanionCoreTests"
    ),
    .testTarget(
      name: "BridgethingCompanionTests",
      dependencies: ["BridgethingCompanion", "BridgethingCompanionCore"],
      path: "packages/companion/swift/Tests/BridgethingCompanionTests"
    ),
  ] + companionCoreTargets
)
