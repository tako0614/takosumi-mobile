import Foundation
import SwiftRs
import Tauri
import UIKit
import WebKit

private let sharedProfile = "shared"
private let keyMaxLength = 512
private let titleMaxLength = 128
private let scopeMaxLength = 512

private class BannerClientArgs: Decodable {
    let name: String
    let verified: Bool
    let appLink: String?
    let universalLink: String?
    let open: String?
}

private class OpenScreenArgs: Decodable {
    let key: String
    let url: String
    let title: String
    let iconPng: String?
    let profile: String
    let accountsOrigins: [String]?
    let scope: String?
    let resume: Bool?
    let client: BannerClientArgs?
}

private class OpenClientArgs: Decodable {
    let platform: String
    let appLink: String?
    let universalLink: String?
    let open: String?
}

private class BrowserViewArgs: Decodable {
    let url: String
    let toolbarColor: String?
}

/// Shell-facing commands; the screens themselves are `AppScreenViewController`.
/// Only the shell's `main` window has the capability for them. App screens are
/// plain WKWebViews without script message handlers, so pages there cannot
/// call these.
final class AppScreenPlugin: Plugin {
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

    /// Hand a screen to its declared native app: the universal link only if an
    /// app takes it (`universalLinksOnly`, never Safari), then the custom
    /// scheme. `canOpenURL` is not used (it needs every scheme listed in
    /// Info.plist). `opened` or `not_installed`.
    @objc func openClient(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(OpenClientArgs.self)
        let universalLink = args.universalLink.flatMap {
            AppScreenPolicy.isHttpsLink($0) ? URL(string: $0) : nil
        }
        let scheme = args.open.flatMap {
            AppScreenPolicy.isCustomSchemeLink($0) ? URL(string: $0) : nil
        }
        guard args.platform == "ios", args.appLink == nil,
              (args.universalLink == nil) == (universalLink == nil),
              (args.open == nil) == (scheme == nil),
              universalLink != nil || scheme != nil else {
            throw requestError("This native app cannot be opened.")
        }
        DispatchQueue.main.async {
            AppClientOpener.open(universalLink: universalLink, scheme: scheme) { opened in
                invoke.resolve(["result": opened ? "opened" : "not_installed"])
            }
        }
    }

    /// Open an https page in `SFSafariViewController` over the shell.
    @objc func openBrowserView(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(BrowserViewArgs.self)
        guard AppScreenPolicy.isLaunchable(args.url), let url = URL(string: args.url) else {
            throw requestError("Only https URLs can be opened in the browser view.")
        }
        if let color = args.toolbarColor, BrowserView.color(hex: color) == nil {
            throw requestError("Toolbar color is invalid.")
        }
        let tint = args.toolbarColor.flatMap { BrowserView.color(hex: $0) }
        DispatchQueue.main.async {
            guard let presenter = self.manager.viewController else {
                invoke.reject("The browser view could not be opened.")
                return
            }
            BrowserView.present(url, from: presenter, barTint: tint)
            invoke.resolve(["result": "browser-view"])
        }
    }

    /// Open an https page with the system (Safari). iOS has no way to keep an
    /// app that claims the URL as a universal link from taking it.
    @objc func openWeb(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(BrowserViewArgs.self)
        guard AppScreenPolicy.isLaunchable(args.url), let url = URL(string: args.url) else {
            throw requestError("Only https URLs can be opened in the browser.")
        }
        DispatchQueue.main.async {
            UIApplication.shared.open(url, options: [:]) { opened in
                if opened {
                    invoke.resolve(["result": "browser"])
                } else {
                    invoke.reject("No browser can open this page.")
                }
            }
        }
    }

    /// The action an app screen's bar left for the shell, once.
    @objc func takePendingAction(_ invoke: Invoke) {
        // The bar writes the action on the main thread; take it there too.
        DispatchQueue.main.async {
            var response: JsonObject = [:]
            if let action = AppScreenActions.take() { response["action"] = action.json }
            invoke.resolve(response)
        }
    }

    /// Tell the shell an action waits (`actionPending`).
    func notifyActionPending() {
        let payload: JSObject = [:]
        trigger("actionPending", data: payload)
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
        let accountsOrigins = args.accountsOrigins ?? []
        guard accountsOrigins.count <= 32, accountsOrigins.allSatisfy({ AppScreenPolicy.isHttpsOrigin($0) }) else {
            throw requestError("The Takosumi origins must be at most 32 https origins.")
        }
        if let scope = args.scope,
           scope.isEmpty || scope.count > scopeMaxLength ||
           scope.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) {
            throw requestError("App screen scope is invalid.")
        }
        var client: AppScreenBannerClient?
        if let declared = args.client {
            guard let checked = AppScreenBannerClient.checked(
                name: declared.name,
                verified: declared.verified,
                hasAndroidLink: declared.appLink != nil,
                universalLink: declared.universalLink,
                open: declared.open)
            else {
                throw requestError("The native app offered by the bar is invalid.")
            }
            client = checked
        }
        let trimmedTitle = String(args.title.trimmingCharacters(in: .whitespacesAndNewlines).prefix(titleMaxLength))
        let title = trimmedTitle.isEmpty ? AppScreenPolicy.displayHost(url) : trimmedTitle
        return AppScreenRequest(
            key: args.key,
            url: url,
            title: title,
            accountsOrigins: accountsOrigins,
            scope: args.scope,
            resume: args.resume ?? false,
            client: client)
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
    let plugin = AppScreenPlugin()
    AppScreenActions.plugin = plugin
    return plugin
}
