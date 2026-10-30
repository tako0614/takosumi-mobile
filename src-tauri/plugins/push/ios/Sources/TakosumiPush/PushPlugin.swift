import Foundation
import ObjectiveC
import Tauri
import TakosumiPushKeychain
import UIKit
import UserNotifications
import WebKit

// Takosumi Mobile's own notifications on iOS (TASK-0119 step 7, code only:
// building and checking it needs macOS and Xcode). The relay's APNs alert
// carries placeholder text and `mutable-content: 1`; replacing the text is the
// Notification Service Extension's job (step 8), which reads the same Keychain
// items and decrypts with the same code (TakosumiPushKeychain). The envelope
// (`s`, `b`) is set by the relay and is not authenticated, so a tap decrypts
// `b` with the subscription's key and takes the instance, the account and the
// page only from a message that decrypts; anything else just opens the app.

private let notificationTappedEvent = "notification-tapped"
private let notificationReceivedEvent = "notification-received"
private let tokenRefreshEvent = "token-refresh"
private let tokenRequestTimeoutSeconds: TimeInterval = 20
private let pendingTapLifetime: TimeInterval = 5 * 60
private let maxLabelLength = 128
private let maxAccountLength = 512

private final class CreateKeysArgs: Decodable {
    let subscriptionId: String
    let instance: String
    let account: String?
    let label: String
    let origins: [String]
}

private final class SubscriptionArgs: Decodable {
    let subscriptionId: String
}

private typealias DidRegisterFunction = @convention(c) (AnyObject, Selector, UIApplication, NSData) -> Void
private typealias DidFailFunction = @convention(c) (AnyObject, Selector, UIApplication, NSError) -> Void
private var installedDelegateClass: AnyClass?

/// The device token arrives through the app delegate, which Tauri owns; add
/// the two callbacks to its class, keeping any existing implementation.
private func installApnsDelegateCallbacks() {
    guard let delegate = UIApplication.shared.delegate,
          let delegateClass: AnyClass = object_getClass(delegate as AnyObject)
    else { return }
    if let installed = installedDelegateClass, installed === delegateClass { return }

    let registerSelector = sel_registerName("application:didRegisterForRemoteNotificationsWithDeviceToken:")
    let registerMethod = class_getInstanceMethod(delegateClass, registerSelector)
    let previousRegister = registerMethod.map { method_getImplementation($0) }
    let register: @convention(block) (AnyObject, UIApplication, NSData) -> Void = { receiver, application, token in
        if let previous = previousRegister {
            unsafeBitCast(previous, to: DidRegisterFunction.self)(receiver, registerSelector, application, token)
        }
        PushPlugin.instance?.didRegister(token as Data)
    }
    _ = class_replaceMethod(
        delegateClass, registerSelector, imp_implementationWithBlock(register as Any),
        registerMethod.flatMap { method_getTypeEncoding($0) } ?? "v@:@@")

    let failSelector = sel_registerName("application:didFailToRegisterForRemoteNotificationsWithError:")
    let failMethod = class_getInstanceMethod(delegateClass, failSelector)
    let previousFail = failMethod.map { method_getImplementation($0) }
    let fail: @convention(block) (AnyObject, UIApplication, NSError) -> Void = { receiver, application, error in
        if let previous = previousFail {
            unsafeBitCast(previous, to: DidFailFunction.self)(receiver, failSelector, application, error)
        }
        PushPlugin.instance?.didFailToRegister()
    }
    _ = class_replaceMethod(
        delegateClass, failSelector, imp_implementationWithBlock(fail as Any),
        failMethod.flatMap { method_getTypeEncoding($0) } ?? "v@:@@")
    installedDelegateClass = delegateClass
}

/// Sees taps on Takosumi pushes, then hands everything to the delegate that
/// was there before (tauri-plugin-notification's, for local notifications).
private final class PushCenterDelegate: NSObject, UNUserNotificationCenterDelegate {
    static let shared = PushCenterDelegate()
    weak var forwarded: UNUserNotificationCenterDelegate?

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        if notification.request.trigger is UNPushNotificationTrigger {
            PushPlugin.instance?.received(notification.request.content.userInfo)
        }
        if let forwarded = forwarded,
           forwarded.responds(to: #selector(UNUserNotificationCenterDelegate.userNotificationCenter(_:willPresent:withCompletionHandler:))) {
            forwarded.userNotificationCenter?(center, willPresent: notification, withCompletionHandler: completionHandler)
        } else {
            completionHandler([.banner, .list, .sound])
        }
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        if response.notification.request.trigger is UNPushNotificationTrigger {
            PushPlugin.instance?.tapped(response.notification.request.content.userInfo)
        }
        if let forwarded = forwarded,
           forwarded.responds(to: #selector(UNUserNotificationCenterDelegate.userNotificationCenter(_:didReceive:withCompletionHandler:))) {
            forwarded.userNotificationCenter?(center, didReceive: response, withCompletionHandler: completionHandler)
        } else {
            completionHandler()
        }
    }
}

@objc(PushPlugin)
public final class PushPlugin: Plugin {
    fileprivate static var instance: PushPlugin?

    private let lock = NSLock()
    private var pendingTap: JSObject?
    private var pendingTapAt = Date.distantPast
    private var currentToken: String?
    private var tokenInvokes: [Invoke] = []

    override public func load(webview: WKWebView) {
        PushPlugin.instance = self
        configure()
    }

    fileprivate func configure() {
        let setUp = {
            installApnsDelegateCallbacks()
            let center = UNUserNotificationCenter.current()
            let delegate = PushCenterDelegate.shared
            if center.delegate !== delegate {
                delegate.forwarded = center.delegate
                center.delegate = delegate
            }
        }
        if Thread.isMainThread { setUp() } else { DispatchQueue.main.async(execute: setUp) }
    }

    @objc public func permissionState(_ invoke: Invoke) {
        UNUserNotificationCenter.current().getNotificationSettings { settings in
            let state: String
            switch settings.authorizationStatus {
            case .authorized, .provisional, .ephemeral: state = "granted"
            case .denied: state = "denied"
            default: state = "prompt"
            }
            invoke.resolve(["state": state])
        }
    }

    @objc public func requestPermission(_ invoke: Invoke) {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .badge, .sound]) { granted, _ in
            invoke.resolve(["granted": granted])
        }
    }

    @objc public func getToken(_ invoke: Invoke) {
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return invoke.reject("APNs bridge is unavailable.") }
            let environment = apnsEnvironment()
            self.lock.lock()
            if let token = self.currentToken {
                self.lock.unlock()
                return invoke.resolve(tokenPayload(token, environment))
            }
            self.tokenInvokes.append(invoke)
            self.lock.unlock()
            DispatchQueue.main.asyncAfter(deadline: .now() + tokenRequestTimeoutSeconds) { [weak self] in
                self?.timeOut(invoke)
            }
            installApnsDelegateCallbacks()
            UIApplication.shared.registerForRemoteNotifications()
        }
    }

    @objc public func deviceName(_ invoke: Invoke) {
        DispatchQueue.main.async {
            // iOS 16+ returns the model name ("iPhone") without a special entitlement.
            invoke.resolve(["name": String(UIDevice.current.name.prefix(maxLabelLength))])
        }
    }

    @objc public func createSubscriptionKeys(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(CreateKeysArgs.self)
        let account = args.account ?? ""
        let label = args.label.trimmingCharacters(in: .whitespacesAndNewlines)
        guard isSubscriptionId(args.subscriptionId),
              isOrigin(args.instance),
              account.count <= maxAccountLength,
              !label.isEmpty, label.count <= maxLabelLength,
              (1...4).contains(args.origins.count),
              args.origins.allSatisfy(isOrigin),
              args.origins.contains(args.instance)
        else { return invoke.reject("invalid subscription keys request") }
        do {
            let subscription = try TakosumiPushKeychain.create(
                subscriptionId: args.subscriptionId,
                instance: args.instance,
                account: account,
                label: label,
                origins: args.origins)
            invoke.resolve([
                "p256dh": base64Url(subscription.publicKey),
                "auth": base64Url(subscription.authSecret),
            ])
        } catch {
            invoke.reject("Could not create subscription keys.")
        }
    }

    @objc public func deleteSubscriptionKeys(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(SubscriptionArgs.self)
        let subscriptionId = args.subscriptionId
        guard isSubscriptionId(subscriptionId) else { return invoke.reject("subscriptionId is invalid") }
        TakosumiPushKeychain.delete(subscriptionId)
        // Its notifications go too (Android deletes the instance's channel).
        let center = UNUserNotificationCenter.current()
        center.getDeliveredNotifications { delivered in
            let identifiers = delivered
                .filter { ($0.request.content.userInfo["s"] as? String) == subscriptionId }
                .map { $0.request.identifier }
            if !identifiers.isEmpty { center.removeDeliveredNotifications(withIdentifiers: identifiers) }
        }
        invoke.resolve()
    }

    @objc public func listSubscriptions(_ invoke: Invoke) {
        let list: [JSObject] = TakosumiPushKeychain.list().map {
            [
                "subscriptionId": $0.subscriptionId,
                "instance": $0.instance,
                "account": $0.account,
                "label": $0.label,
            ]
        }
        invoke.resolve(["subscriptions": list])
    }

    @objc public func takePendingTap(_ invoke: Invoke) {
        lock.lock()
        // A tap the shell did not take within a few minutes is not used.
        let tap = Date().timeIntervalSince(pendingTapAt) <= pendingTapLifetime ? pendingTap : nil
        pendingTap = nil
        lock.unlock()
        if let tap = tap { invoke.resolve(["tap": tap]) } else { invoke.resolve([:]) }
    }

    /// The subscription and message of a push, only when `b` decrypts with
    /// that subscription's key and `auth` and is a Takosumi notification.
    private func authenticated(
        _ userInfo: [AnyHashable: Any]
    ) -> (TakosumiPushSubscription, TakosumiNotification)? {
        guard let subscriptionId = userInfo["s"] as? String,
              isSubscriptionId(subscriptionId),
              let encoded = userInfo["b"] as? String,
              let body = TakosumiPushOrigin.base64UrlDecode(encoded),
              let subscription = TakosumiPushKeychain.find(subscriptionId),
              let plaintext = try? TakosumiWebPush.decrypt(body, subscription: subscription),
              let notification = TakosumiWebPush.parse(plaintext, origins: subscription.origins)
        else { return nil }
        return (subscription, notification)
    }

    fileprivate func tapped(_ userInfo: [AnyHashable: Any]) {
        guard let message = authenticated(userInfo) else { return }
        let (subscription, notification) = message
        var tap: JSObject = [
            "id": UUID().uuidString,
            "subscriptionId": subscription.subscriptionId,
            "instance": subscription.instance,
            "account": subscription.account,
        ]
        if let navigate = notification.navigate { tap["navigate"] = navigate }
        lock.lock()
        pendingTap = tap
        pendingTapAt = Date()
        lock.unlock()
        trigger(notificationTappedEvent, data: tap)
    }

    fileprivate func received(_ userInfo: [AnyHashable: Any]) {
        guard let subscription = authenticated(userInfo)?.0 else { return }
        trigger(notificationReceivedEvent, data: [
            "subscriptionId": subscription.subscriptionId,
            "instance": subscription.instance,
            "account": subscription.account,
        ])
    }

    fileprivate func didRegister(_ data: Data) {
        guard !data.isEmpty else { return didFailToRegister() }
        let environment = apnsEnvironment()
        let token = data.map { String(format: "%02x", $0) }.joined()
        lock.lock()
        let changed = currentToken != nil && currentToken != token
        currentToken = token
        let invokes = tokenInvokes
        tokenInvokes.removeAll()
        lock.unlock()
        let payload = tokenPayload(token, environment)
        for invoke in invokes { invoke.resolve(payload) }
        if changed { trigger(tokenRefreshEvent, data: payload) }
    }

    fileprivate func didFailToRegister() {
        lock.lock()
        let invokes = tokenInvokes
        tokenInvokes.removeAll()
        lock.unlock()
        // Without the Push Notifications capability (aps-environment) iOS
        // refuses the registration: this build cannot receive pushes.
        for invoke in invokes { invoke.reject("push_unavailable: APNs registration failed") }
    }

    private func timeOut(_ invoke: Invoke) {
        lock.lock()
        let index = tokenInvokes.firstIndex { $0 === invoke }
        if let index = index { tokenInvokes.remove(at: index) }
        lock.unlock()
        if index != nil { invoke.reject("APNs device token request timed out.") }
    }
}

private func tokenPayload(_ token: String, _ environment: String) -> JSObject {
    ["token": token, "provider": "apns", "environment": environment]
}

/// The APNs environment this build's tokens belong to, as the relay names it
/// (`development` is the sandbox): Info.plist `TakosumiApnsEnvironment` when
/// the build configuration sets it; else the `aps-environment` entitlement in
/// the embedded provisioning profile (development and ad-hoc builds); else
/// production, since App Store and TestFlight builds carry no profile and
/// always get production tokens. (The SecTask entitlement API is macOS-only.)
private func apnsEnvironment() -> String {
    if let configured = Bundle.main.object(forInfoDictionaryKey: "TakosumiApnsEnvironment") as? String,
       configured == "development" || configured == "production" {
        return configured
    }
    return provisionedApnsEnvironment() ?? "production"
}

private func provisionedApnsEnvironment() -> String? {
    guard let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision"),
          let data = try? Data(contentsOf: url),
          let start = data.range(of: Data("<?xml".utf8)),
          let end = data.range(of: Data("</plist>".utf8), in: start.lowerBound..<data.endIndex),
          let plist = (try? PropertyListSerialization.propertyList(
              from: data.subdata(in: start.lowerBound..<end.upperBound), format: nil)) as? [String: Any],
          let entitlements = plist["Entitlements"] as? [String: Any],
          let value = entitlements["aps-environment"] as? String
    else { return nil }
    return value == "development" || value == "production" ? value : nil
}

private func isSubscriptionId(_ value: String) -> Bool {
    (8...128).contains(value.count) && value.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-" || $0 == "_") }
}

private func isOrigin(_ value: String) -> Bool {
    guard value.count <= 256, value == value.lowercased(), let origin = TakosumiPushOrigin.of(value) else { return false }
    return origin == value
}

private func base64Url(_ data: Data) -> String {
    data.base64EncodedString()
        .replacingOccurrences(of: "+", with: "-")
        .replacingOccurrences(of: "/", with: "_")
        .replacingOccurrences(of: "=", with: "")
}

@_cdecl("init_plugin_takosumi_push")
func initPlugin() -> Plugin {
    let plugin = PushPlugin()
    PushPlugin.instance = plugin
    plugin.configure()
    return plugin
}
