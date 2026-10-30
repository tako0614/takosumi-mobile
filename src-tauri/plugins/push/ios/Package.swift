// swift-tools-version:5.3

import PackageDescription

let package = Package(
    name: "tauri-plugin-takosumi-push",
    platforms: [
        .iOS(.v14),
    ],
    products: [
        .library(
            name: "tauri-plugin-takosumi-push",
            type: .static,
            targets: ["TakosumiPush"]),
        // The subscription keys in the shared Keychain access group, for the
        // Notification Service Extension (TASK-0119 step 8) to link as well.
        .library(
            name: "TakosumiPushKeychain",
            targets: ["TakosumiPushKeychain"]),
    ],
    dependencies: [
        .package(name: "Tauri", path: "../.tauri/tauri-api"),
    ],
    targets: [
        .target(
            name: "TakosumiPushKeychain",
            path: "Sources/TakosumiPushKeychain"),
        .target(
            name: "TakosumiPush",
            dependencies: [.byName(name: "Tauri"), "TakosumiPushKeychain"],
            path: "Sources/TakosumiPush"),
    ]
)
