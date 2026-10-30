// swift-tools-version:5.7

// The plugin's RFC 8291 decryption and Declarative Web Push parsing
// (`../Sources/TakosumiPushKeychain/TakosumiWebPush.swift`, linked here)
// against `web-push-cases.json`, the table the Android plugin and
// `tests/push-cases.test.ts` read. A package of its own so it builds without
// UIKit, the Keychain or the Tauri API:
// `swift test --package-path src-tauri/plugins/push/ios/WebPushTests` runs on
// macOS (CryptoKit) and Linux (swift-crypto, the same API).

import PackageDescription

let package = Package(
    name: "TakosumiWebPushTests",
    platforms: [.macOS(.v10_15)],
    dependencies: [
        .package(url: "https://github.com/apple/swift-crypto.git", exact: "4.5.2"),
    ],
    targets: [
        .target(
            name: "TakosumiWebPushCore",
            dependencies: [
                .product(name: "Crypto", package: "swift-crypto", condition: .when(platforms: [.linux])),
            ],
            path: "Sources/TakosumiWebPushCore"),
        .testTarget(
            name: "TakosumiWebPushTests",
            dependencies: ["TakosumiWebPushCore"],
            path: "Tests/TakosumiWebPushTests"),
    ]
)
