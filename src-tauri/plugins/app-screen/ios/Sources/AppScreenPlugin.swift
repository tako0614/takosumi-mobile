import Foundation
import SwiftRs
import Tauri
import UIKit
import WebKit

private let sharedProfile = "shared"
private let keyMaxLength = 512
private let titleMaxLength = 128

private class OpenScreenArgs: Decodable {
    let key: String
    let url: String
    let title: String
    let iconPng: String?
    let profile: String
}

/// Shell-facing commands; the screens themselves are `AppScreenViewController`.
private final class AppScreenPlugin: Plugin {
    @objc func capabilities(_ invoke: Invoke) {
        // iOS has no API for an app to add a home-screen icon; the widget and
        // App Intents (TASK-0119 U4 step 5) take that role.
        invoke.resolve(["pinShortcut": false])
    }

    @objc func open(_ invoke: Invoke) throws {
        let request = try screenRequest(invoke)
        DispatchQueue.main.async {
            guard let presenter = self.manager.viewController else {
                invoke.reject("The app screen could not be opened.")
                return
            }
            AppScreenCoordinator.shared.open(request, from: presenter)
            invoke.resolve()
        }
    }

    @objc func pinShortcut(_ invoke: Invoke) throws {
        _ = try screenRequest(invoke)
        invoke.resolve(["requested": false])
    }

    private func screenRequest(_ invoke: Invoke) throws -> AppScreenRequest {
        let args = try invoke.parseArgs(OpenScreenArgs.self)
        guard args.profile == sharedProfile else {
            throw requestError("App screens share one browsing profile.")
        }
        let key = args.key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty, args.key.count <= keyMaxLength else {
            throw requestError("App screen key is invalid.")
        }
        guard AppScreenPolicy.isLaunchable(args.url), let url = URL(string: args.url) else {
            throw requestError("Only https URLs can be opened in an app screen.")
        }
        let trimmedTitle = String(args.title.trimmingCharacters(in: .whitespacesAndNewlines).prefix(titleMaxLength))
        let title = trimmedTitle.isEmpty ? AppScreenPolicy.displayHost(url) : trimmedTitle
        return AppScreenRequest(key: args.key, url: url, title: title)
    }

    private func requestError(_ message: String) -> NSError {
        NSError(
            domain: "com.takosumi.appscreen",
            code: -1,
            userInfo: [NSLocalizedDescriptionKey: message])
    }
}

@_cdecl("init_plugin_app_screen")
func initPlugin() -> Plugin {
    AppScreenPlugin()
}
