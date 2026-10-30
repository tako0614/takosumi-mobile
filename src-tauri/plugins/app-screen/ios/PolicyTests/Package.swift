// swift-tools-version:5.7

// The app-screen navigation policy (`../Sources/AppScreenPolicy.swift`, linked
// here) against the tables `src/url-policy.ts` and the Android policy are
// tested with. A package of its own so it builds without UIKit or the Tauri
// API: `swift test --package-path src-tauri/plugins/app-screen/ios/PolicyTests`
// runs on macOS and Linux.

import PackageDescription

let package = Package(
    name: "AppScreenPolicyTests",
    targets: [
        .target(name: "AppScreenPolicyCore", path: "Sources/AppScreenPolicyCore"),
        .testTarget(
            name: "AppScreenPolicyTests",
            dependencies: ["AppScreenPolicyCore"],
            path: "Tests/AppScreenPolicyTests"),
    ]
)
