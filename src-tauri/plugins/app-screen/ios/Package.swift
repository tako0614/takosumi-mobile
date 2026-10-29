// swift-tools-version:5.3

import PackageDescription

let package = Package(
    name: "tauri-plugin-app-screen",
    platforms: [
        .macOS(.v10_13),
        .iOS(.v14),
    ],
    products: [
        .library(
            name: "tauri-plugin-app-screen",
            type: .static,
            targets: ["tauri-plugin-app-screen"]),
    ],
    dependencies: [
        .package(name: "Tauri", path: "../.tauri/tauri-api"),
    ],
    targets: [
        .target(
            name: "tauri-plugin-app-screen",
            dependencies: [.byName(name: "Tauri")],
            path: "Sources"),
    ]
)
