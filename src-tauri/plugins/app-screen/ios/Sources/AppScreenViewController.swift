import SafariServices
import UIKit
import WebKit

/// The declared native app the bar offers ("{name}で開く"). A verified one has
/// its links; an unverified one has none and sends the person to the chooser.
struct AppScreenBannerClient {
    let name: String
    let verified: Bool
    let universalLink: URL?
    let open: URL?

    /// Re-checks everything; an invalid client is dropped, not repaired.
    static func checked(
        name: String,
        verified: Bool,
        hasAndroidLink: Bool,
        universalLink: String?,
        open: String?
    ) -> AppScreenBannerClient? {
        guard AppScreenPolicy.isClientName(name), !hasAndroidLink else { return nil }
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if !verified {
            guard universalLink == nil, open == nil else { return nil }
            return AppScreenBannerClient(name: trimmed, verified: false, universalLink: nil, open: nil)
        }
        let link = universalLink.flatMap { AppScreenPolicy.isHttpsLink($0) ? URL(string: $0) : nil }
        let scheme = open.flatMap { AppScreenPolicy.isCustomSchemeLink($0) ? URL(string: $0) : nil }
        guard (universalLink == nil) == (link == nil), (open == nil) == (scheme == nil),
              link != nil || scheme != nil else {
            return nil
        }
        return AppScreenBannerClient(name: trimmed, verified: true, universalLink: link, open: scheme)
    }
}

struct AppScreenRequest {
    let key: String
    let url: URL
    let title: String
    /// The Takosumi origins (the instance's Accounts and the instance itself):
    /// the screen shows only their sign-in pages.
    var accountsOrigins: [String] = []
    /// (instance, account) the last page is remembered for.
    var scope: String?
    /// Reopen at the last remembered page instead of `url`.
    var resume = false
    var client: AppScreenBannerClient?

    var appOrigin: String? { AppScreenPolicy.origin(of: url.absoluteString) }

    /// What this screen shows; everything else goes to the browser view.
    var origins: AppScreenOrigins {
        AppScreenOrigins(appOrigin: appOrigin, accountsOrigins: Set(accountsOrigins))
    }
}

final class AppScreenSession {
    let key: String
    let webView: WKWebView
    let popup: Bool
    let navigator: AppScreenNavigator
    /// URL the screen was opened with; reopening with it keeps the page state.
    var launchURL: URL?
    var title: String = ""
    var scope: String?
    var client: AppScreenBannerClient?

    init(key: String, webView: WKWebView, popup: Bool) {
        self.key = key
        self.webView = webView
        self.popup = popup
        navigator = AppScreenNavigator(webView: webView)
    }
}

/// Hands a screen to its declared native app (`open_client`).
enum AppClientOpener {
    static func open(universalLink: URL?, scheme: URL?, completion: @escaping (Bool) -> Void) {
        func openScheme() {
            guard let scheme = scheme else {
                completion(false)
                return
            }
            // `false` when no installed app handles the scheme.
            UIApplication.shared.open(scheme, options: [:]) { completion($0) }
        }
        guard let link = universalLink else {
            openScheme()
            return
        }
        // Opens only an installed app that claims the link, never Safari.
        UIApplication.shared.open(link, options: [.universalLinksOnly: true]) { opened in
            if opened { completion(true) } else { openScheme() }
        }
    }
}

/// The browser view: `SFSafariViewController` over Takosumi Mobile, with the
/// person's Safari sign-ins. External sign-ins (Google forbids OAuth in
/// embedded webviews) go here; closing it comes back to the screen.
enum BrowserView {
    static let barColor = UIColor(red: 0x15 / 255, green: 0x15 / 255, blue: 0x17 / 255, alpha: 1)

    static func present(_ url: URL, from presenter: UIViewController, barTint: UIColor? = nil) {
        guard AppScreenPolicy.isLaunchable(url.absoluteString) else { return }
        let safari = SFSafariViewController(url: url)
        safari.preferredBarTintColor = barTint ?? barColor
        safari.preferredControlTintColor = .white
        safari.dismissButtonStyle = .close
        var top = presenter
        while let presented = top.presentedViewController, !presented.isBeingDismissed {
            top = presented
        }
        top.present(safari, animated: true)
    }

    static func color(hex: String) -> UIColor? {
        guard hex.count == 7, hex.hasPrefix("#"), let value = UInt32(hex.dropFirst(), radix: 16) else {
            return nil
        }
        return UIColor(
            red: CGFloat((value >> 16) & 0xff) / 255,
            green: CGFloat((value >> 8) & 0xff) / 255,
            blue: CGFloat(value & 0xff) / 255,
            alpha: 1)
    }
}

/// The last page of each app per (instance, account), so reopening an app
/// starts where the person left it. Only pages `isRememberable` accepts are
/// kept: the app's own origin, no credentials.
enum AppScreenResume {
    private static let defaultsKey = "com.takosumi.appscreen.resume.v1"
    private static let maxRemembered = 64

    static func record(_ url: URL, key: String, scope: String?, appOrigin: String?) {
        guard let scope = scope, let appOrigin = appOrigin,
              AppScreenPolicy.isRememberable(url.absoluteString, appOrigin: appOrigin) else {
            return
        }
        var stored = UserDefaults.standard.dictionary(forKey: defaultsKey) as? [String: [String: Any]] ?? [:]
        let id = identifier(scope: scope, key: key)
        if stored[id]?["url"] as? String == url.absoluteString { return }
        stored[id] = ["url": url.absoluteString, "t": Date().timeIntervalSince1970]
        if stored.count > maxRemembered {
            let oldest = stored.sorted { ($0.value["t"] as? Double ?? 0) < ($1.value["t"] as? Double ?? 0) }
            for (staleId, _) in oldest.prefix(stored.count - maxRemembered) { stored.removeValue(forKey: staleId) }
        }
        UserDefaults.standard.set(stored, forKey: defaultsKey)
    }

    static func recall(_ request: AppScreenRequest) -> URL? {
        guard let scope = request.scope, let appOrigin = request.appOrigin else { return nil }
        let stored = UserDefaults.standard.dictionary(forKey: defaultsKey) as? [String: [String: Any]]
        guard let raw = stored?[identifier(scope: scope, key: request.key)]?["url"] as? String,
              AppScreenPolicy.isRememberable(raw, appOrigin: appOrigin) else {
            return nil
        }
        return URL(string: raw)
    }

    private static func identifier(scope: String, key: String) -> String {
        // Scope and key are bounded and printable; the pair needs no hashing.
        "\(scope)\n\(key)"
    }
}

/// What an app screen's bar asks the shell to do: show the chooser for an app.
struct AppScreenPendingAction {
    let key: String
    let url: String
    let reason: String

    var json: [String: Any] { ["kind": "choose", "key": key, "url": url, "reason": reason] }
}

/// The bar's actions for the shell (開き方を変える, a native app that is not
/// installed, an unverified one to confirm). The action waits here until the
/// shell takes it (`take_pending_action`); the plugin's `actionPending` event
/// tells the shell, which the app screen is dismissed to.
enum AppScreenActions {
    static let change = "change"
    static let notInstalled = "not-installed"
    static let unverified = "unverified"

    static weak var plugin: AppScreenPlugin?
    private static var pending: AppScreenPendingAction?

    static func take() -> AppScreenPendingAction? {
        defer { pending = nil }
        return pending
    }

    static func request(_ action: AppScreenPendingAction) {
        guard AppScreenPolicy.isLaunchable(action.url) else { return }
        pending = action
        plugin?.notifyActionPending()
    }
}

/// Calls WebKit's completion handler exactly once, also when the dialog is
/// dismissed from code (WebKit raises if a handler is dropped uncalled).
final class DialogCompletion {
    private var finished = false
    private let onCancel: () -> Void

    init(onCancel: @escaping () -> Void) {
        self.onCancel = onCancel
    }

    func finish(_ action: () -> Void) {
        guard !finished else { return }
        finished = true
        action()
    }

    func cancel() {
        finish(onCancel)
    }
}

/// Navigation and UI delegate for one session, attached for its whole life,
/// so the policy holds while the screen is not shown. UI (prompts, popups,
/// bar updates) goes to the controller currently showing the session.
final class AppScreenNavigator: NSObject, WKNavigationDelegate, WKUIDelegate {
    weak var screen: AppScreenViewController?
    /// What the main frame may show (the app's origin, the Takosumi origins'
    /// sign-in pages); other https pages go to the browser view.
    var origins: AppScreenOrigins?
    /// Called with each committed page the screen shows, to remember where to
    /// reopen; only set for an app opened as itself, never for a deep link.
    var onCommitted: ((URL) -> Void)?
    /// Where to go when a page the screen does not show was reached in place.
    var fallbackURL: URL?
    private var lastDownloadPromptAt: Date?
    /// Last committed URL. The host label shows only this, never the URL of
    /// a navigation that has not committed yet (no spoofing by slow loads).
    private(set) var committedURL: URL?
    private(set) var needsReload = false
    private var observations: [NSKeyValueObservation] = []

    init(webView: WKWebView) {
        super.init()
        webView.navigationDelegate = self
        webView.uiDelegate = self
        observations = [
            webView.observe(\.url, options: [.new]) { [weak self] webView, _ in
                self?.urlChanged(webView)
            },
            webView.observe(\.isLoading, options: [.new]) { [weak self] webView, _ in
                self?.refreshCommitted(webView)
                self?.screen?.updateProgress()
            },
            webView.observe(\.canGoBack, options: [.new]) { [weak self] _, _ in
                self?.screen?.updateBar()
            },
            webView.observe(\.estimatedProgress, options: [.new]) { [weak self] _, _ in
                self?.screen?.updateProgress()
            },
        ]
    }

    func invalidate(_ webView: WKWebView) {
        observations.removeAll()
        webView.navigationDelegate = nil
        webView.uiDelegate = nil
        screen = nil
    }

    func reloadIfNeeded(_ webView: WKWebView) {
        guard needsReload else { return }
        needsReload = false
        // A load just started (a deep link) replaces the crashed page anyway.
        guard !webView.isLoading else { return }
        webView.reload()
    }

    /// Every URL change, also while a load is in flight: a page the screen
    /// does not show (a Takosumi page outside sign-in) is hidden at once and
    /// reverted, never displayed.
    private func urlChanged(_ webView: WKWebView) {
        if let url = webView.url, !shows(url) {
            _ = keepsCommitted(webView, url)
            return
        }
        refreshCommitted(webView)
    }

    private func shows(_ url: URL) -> Bool {
        let raw = url.absoluteString
        return raw == "about:blank" || raw == "about:srcdoc" ||
            AppScreenPolicy.isInScreen(raw, origins: origins)
    }

    /// While a load is in flight, `url` may already be the next page's.
    private func refreshCommitted(_ webView: WKWebView) {
        if !webView.isLoading, let url = webView.url {
            guard keepsCommitted(webView, url) else { return }
            committedURL = url
            onCommitted?(url)
        } else if !webView.isLoading {
            committedURL = webView.url
        }
        screen?.updateBar()
    }

    /// A page the screen shows, else hand it over and go back. URL changes of
    /// a single-page app (`history.pushState`) reach only here: the Takosumi
    /// sign-in screen is the dashboard's app and could move to the dashboard
    /// in place. The webview stays hidden until an allowed page has loaded.
    private func keepsCommitted(_ webView: WKWebView, _ url: URL) -> Bool {
        if shows(url) {
            // Shown again only once an allowed page is in place, not while
            // the revert is still loading.
            if webView.isHidden && !webView.isLoading { webView.isHidden = false }
            return true
        }
        let raw = url.absoluteString
        webView.isHidden = true
        if AppScreenPolicy.decide(raw, mainFrame: true, origins: origins) == .browserView {
            screen?.handOffToBrowserView(url, userGesture: false)
        }
        if webView.canGoBack {
            webView.goBack()
        } else if let fallback = fallbackURL {
            webView.load(URLRequest(url: fallback))
        } else {
            webView.stopLoading()
            webView.load(URLRequest(url: URL(string: "about:blank")!))
        }
        return false
    }

    // MARK: WKNavigationDelegate

    func webView(
        _ webView: WKWebView,
        decidePolicyFor navigationAction: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        let url = navigationAction.request.url
        // A nil target frame is a new window; it is judged as a main frame.
        let mainFrame = navigationAction.targetFrame?.isMainFrame ?? true
        switch AppScreenPolicy.decide(
            url?.absoluteString ?? "", mainFrame: mainFrame, origins: origins)
        {
        case .inApp:
            decisionHandler(.allow)
        case .block:
            decisionHandler(.cancel)
        case .browserView:
            decisionHandler(.cancel)
            guard let url = url else { return }
            if (navigationAction.request.httpMethod ?? "GET").uppercased() == "GET" {
                screen?.handOffToBrowserView(
                    url, userGesture: navigationAction.navigationType == .linkActivated)
            } else {
                // A form POST cannot be handed over as a link.
                screen?.offerBrowserViewForPage(blocked: url)
            }
        case .externalConfirm(let insecureHttp):
            decisionHandler(.cancel)
            if mainFrame, let url = url {
                screen?.confirmExternal(
                    url,
                    insecureHttp: insecureHttp,
                    userGesture: navigationAction.navigationType == .linkActivated)
            }
        }
    }

    func webView(
        _ webView: WKWebView,
        decidePolicyFor navigationResponse: WKNavigationResponse,
        decisionHandler: @escaping (WKNavigationResponsePolicy) -> Void
    ) {
        if !navigationResponse.canShowMIMEType,
           navigationResponse.isForMainFrame,
           let url = navigationResponse.response.url,
           AppScreenPolicy.isLaunchable(url.absoluteString) {
            decisionHandler(.cancel)
            // A page cannot keep raising download prompts.
            if let last = lastDownloadPromptAt, Date().timeIntervalSince(last) < 3 { return }
            lastDownloadPromptAt = Date()
            screen?.confirmExternal(url, insecureHttp: false, download: true, userGesture: true)
            return
        }
        decisionHandler(.allow)
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        if let url = webView.url, !keepsCommitted(webView, url) { return }
        committedURL = webView.url
        screen?.updateBar()
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        refreshCommitted(webView)
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        if screen != nil {
            webView.reload()
        } else {
            needsReload = true
        }
    }

    // MARK: WKUIDelegate

    func webView(
        _ webView: WKWebView,
        createWebViewWith configuration: WKWebViewConfiguration,
        for navigationAction: WKNavigationAction,
        windowFeatures: WKWindowFeatures
    ) -> WKWebView? {
        // `window.open()` reaches here without a policy decision on the opener.
        if let url = navigationAction.request.url, url.absoluteString != "about:blank",
           !url.absoluteString.isEmpty
        {
            switch AppScreenPolicy.decide(url.absoluteString, mainFrame: true, origins: origins) {
            case .inApp:
                break
            case .browserView:
                screen?.handOffToBrowserView(url, userGesture: true)
                return nil
            case .externalConfirm(let insecureHttp):
                screen?.confirmExternal(url, insecureHttp: insecureHttp, userGesture: true)
                return nil
            case .block:
                return nil
            }
        }
        return screen?.presentPopup(configuration: configuration, origins: origins)
    }

    func webViewDidClose(_ webView: WKWebView) {
        screen?.pageClosed(webView)
    }

    func webView(
        _ webView: WKWebView,
        runJavaScriptAlertPanelWithMessage message: String,
        initiatedByFrame frame: WKFrameInfo,
        completionHandler: @escaping () -> Void
    ) {
        let completion = DialogCompletion { completionHandler() }
        guard let screen = screen else {
            completion.cancel()
            return
        }
        screen.presentDialog(
            title: frame.securityOrigin.host, message: message, completion: completion,
            actions: [("OK", .default, { completionHandler() })], textField: nil)
    }

    func webView(
        _ webView: WKWebView,
        runJavaScriptConfirmPanelWithMessage message: String,
        initiatedByFrame frame: WKFrameInfo,
        completionHandler: @escaping (Bool) -> Void
    ) {
        let completion = DialogCompletion { completionHandler(false) }
        guard let screen = screen else {
            completion.cancel()
            return
        }
        screen.presentDialog(
            title: frame.securityOrigin.host, message: message, completion: completion,
            actions: [
                ("キャンセル", .cancel, { completionHandler(false) }),
                ("OK", .default, { completionHandler(true) }),
            ],
            textField: nil)
    }

    func webView(
        _ webView: WKWebView,
        runJavaScriptTextInputPanelWithPrompt prompt: String,
        defaultText: String?,
        initiatedByFrame frame: WKFrameInfo,
        completionHandler: @escaping (String?) -> Void
    ) {
        let completion = DialogCompletion { completionHandler(nil) }
        guard let screen = screen else {
            completion.cancel()
            return
        }
        let field = DialogTextField(defaultText: defaultText)
        screen.presentDialog(
            title: frame.securityOrigin.host, message: prompt, completion: completion,
            actions: [
                ("キャンセル", .cancel, { completionHandler(nil) }),
                ("OK", .default, { completionHandler(field.text) }),
            ],
            textField: field)
    }

    /// Camera and microphone for app screens come later with an origin-named
    /// prompt; until then they are refused (as on Android).
    @available(iOS 15.0, *)
    func webView(
        _ webView: WKWebView,
        requestMediaCapturePermissionFor origin: WKSecurityOrigin,
        initiatedByFrame frame: WKFrameInfo,
        type: WKMediaCaptureType,
        decisionHandler: @escaping (WKPermissionDecision) -> Void
    ) {
        decisionHandler(.deny)
    }
}

/// Text of a prompt dialog's field, read when the person taps OK.
final class DialogTextField {
    let defaultText: String?
    weak var field: UITextField?

    init(defaultText: String?) {
        self.defaultText = defaultText
    }

    var text: String? { field?.text }
}

/// Presents app screens over the Tauri shell. One app screen stays alive at a
/// time (with the popups it opened); another app's page is reopened by URL,
/// at its last remembered page. Keeping several live and switching between
/// them waits for device testing (TASK-0119 step 3).
///
/// Every screen uses the default website data store, so cookies and storage are
/// one profile across apps: a Takosumi sign-in made in one app screen carries
/// over to the others. Do not give screens their own data store.
///
/// The webviews are plain WKWebViews with a fresh configuration: no script
/// message handler, no user script and no URL scheme handler, so a page can
/// reach neither the Tauri IPC nor native plugins.
final class AppScreenCoordinator {
    static let shared = AppScreenCoordinator()
    /// Live app screens, not counting popups.
    static let maxAliveScreens = 1

    private let processPool = WKProcessPool()
    private var sessions: [String: AppScreenSession] = [:]
    private var order: [String] = []
    private var visibleKeys: Set<String> = []
    private weak var screen: AppScreenViewController?

    func open(_ request: AppScreenRequest, from presenter: UIViewController) {
        // A Takosumi page outside sign-in (the dashboard, account pages) never
        // runs in an app screen.
        guard AppScreenPolicy.isInScreen(request.url.absoluteString, origins: request.origins) else {
            BrowserView.present(request.url, from: presenter)
            return
        }
        let existing = sessions[request.key]
        // A live page is kept when the app is reopened (resume) or opened with
        // its own launch URL, for the same (instance, account).
        let keep = existing != nil && existing?.webView.url != nil && existing?.scope == request.scope &&
            (request.resume || existing?.launchURL == request.url)
        let session = existing ?? makeSession(key: request.key, popup: false, configuration: nil)
        if !keep { session.launchURL = request.url }
        session.title = request.title
        session.scope = request.scope
        session.client = request.client
        session.navigator.origins = request.origins
        session.navigator.fallbackURL = request.url
        let key = request.key
        let scope = request.scope
        let appOrigin = request.appOrigin
        // Only an app opened as itself (tile, recent) keeps its last page; a
        // deep-linked page never becomes it.
        session.navigator.onCommitted = request.resume ? { url in
            AppScreenResume.record(url, key: key, scope: scope, appOrigin: appOrigin)
        } : nil
        touch(session.key)
        if !keep {
            let start = (request.resume ? AppScreenResume.recall(request) : nil) ?? request.url
            session.webView.load(URLRequest(url: start))
        }

        if let screen = screen, screen.presentingViewController != nil, !screen.isBeingDismissed {
            screen.dismissPopups()
            screen.show(session)
        } else {
            let controller = AppScreenViewController(coordinator: self, session: session)
            controller.modalPresentationStyle = .fullScreen
            controller.loadViewIfNeeded()
            screen = controller
            topmost(from: presenter).present(controller, animated: true)
        }
        evict(keeping: session.key)
    }

    func makePopupSession(
        configuration: WKWebViewConfiguration,
        origins: AppScreenOrigins?
    ) -> AppScreenSession {
        let session = makeSession(key: "popup:\(UUID().uuidString)", popup: true, configuration: configuration)
        // A popup keeps its opener's origins.
        session.navigator.origins = origins
        return session
    }

    func remove(_ key: String) {
        guard let session = sessions.removeValue(forKey: key) else { return }
        order.removeAll { $0 == key }
        visibleKeys.remove(key)
        session.webView.stopLoading()
        session.navigator.invalidate(session.webView)
        session.webView.removeFromSuperview()
    }

    func setVisible(_ key: String, _ visible: Bool) {
        if visible {
            visibleKeys.insert(key)
            touch(key)
        } else {
            visibleKeys.remove(key)
        }
    }

    private func makeSession(
        key: String,
        popup: Bool,
        configuration: WKWebViewConfiguration?
    ) -> AppScreenSession {
        let webView = WKWebView(frame: .zero, configuration: configuration ?? makeConfiguration())
        webView.allowsBackForwardNavigationGestures = true
        let session = AppScreenSession(key: key, webView: webView, popup: popup)
        sessions[key] = session
        touch(key)
        return session
    }

    private func makeConfiguration() -> WKWebViewConfiguration {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = WKWebsiteDataStore.default()
        // iOS 14 still keeps session cookies per process pool; share one.
        configuration.processPool = processPool
        configuration.allowsInlineMediaPlayback = true
        configuration.mediaTypesRequiringUserActionForPlayback = .all
        configuration.dataDetectorTypes = []
        // Popups need a user gesture, as in a browser.
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        return configuration
    }

    private func touch(_ key: String) {
        order.removeAll { $0 == key }
        order.append(key)
    }

    /// Close every other app screen that is not on screen (`maxAliveScreens`).
    /// Popups are closed with the screen that presented them.
    private func evict(keeping key: String) {
        let victims = order.filter { candidate in
            candidate != key && !visibleKeys.contains(candidate) && sessions[candidate]?.popup == false
        }
        for victim in victims { remove(victim) }
    }

    private func topmost(from controller: UIViewController) -> UIViewController {
        var current = controller
        while let presented = current.presentedViewController, !presented.isBeingDismissed {
            current = presented
        }
        return current
    }
}

/// One installed app in a WKWebView under a thin bar: the current host (always
/// visible; there is no address field), back, reload, close and a menu with
/// share, the browser view, Safari, the app's declared native app,
/// 開き方を変える and switch apps.
final class AppScreenViewController: UIViewController {
    private static let barColor = BrowserView.barColor
    private static let promptCooldown: TimeInterval = 5
    private static let browserViewCooldown: TimeInterval = 3

    private let coordinator: AppScreenCoordinator
    private(set) var session: AppScreenSession
    private var attachedSession: AppScreenSession?
    private var pendingDialog: DialogCompletion?
    private var lastExternalDeclinedAt: Date?
    private var lastBrowserViewAt: Date?

    private let bar = UIStackView()
    private let hostLabel = UILabel()
    private let backButton = UIButton(type: .system)
    private let reloadButton = UIButton(type: .system)
    private let menuButton = UIButton(type: .system)
    private let closeButton = UIButton(type: .system)
    private let progressView = UIProgressView(progressViewStyle: .bar)
    private let container = UIView()

    init(coordinator: AppScreenCoordinator, session: AppScreenSession) {
        self.coordinator = coordinator
        self.session = session
        super.init(nibName: nil, bundle: nil)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    override var preferredStatusBarStyle: UIStatusBarStyle { .lightContent }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Self.barColor
        buildLayout()
        attach(session)
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        coordinator.setVisible(session.key, true)
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        // Covered by its own popup it stays on the stack and stays alive.
        if isBeingDismissed || presentingViewController == nil {
            pendingDialog?.cancel()
            // The app may already be reopened in a new controller.
            if ownsSession() { coordinator.setVisible(session.key, false) }
        }
    }

    func show(_ next: AppScreenSession) {
        coordinator.setVisible(session.key, false)
        session = next
        coordinator.setVisible(next.key, true)
        if isViewLoaded {
            attach(next)
            // The same session may come back with another declared app.
            menuButton.menu = makeMenu()
        }
    }

    /// Close popups opened from this screen (they are transient).
    func dismissPopups() {
        guard presentedViewController != nil else { return }
        removePopupChain()
        dismiss(animated: false)
    }

    private func attach(_ next: AppScreenSession) {
        if attachedSession === next { return }
        if let previous = attachedSession {
            if previous.navigator.screen === self { previous.navigator.screen = nil }
            previous.webView.removeFromSuperview()
        }
        attachedSession = next
        next.navigator.screen = self
        let webView = next.webView
        webView.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(webView)
        NSLayoutConstraint.activate([
            webView.topAnchor.constraint(equalTo: container.topAnchor),
            webView.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            webView.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            webView.trailingAnchor.constraint(equalTo: container.trailingAnchor),
        ])
        menuButton.menu = makeMenu()
        next.navigator.reloadIfNeeded(webView)
        updateBar()
        updateProgress()
    }

    // MARK: Bar

    private func buildLayout() {
        bar.axis = .horizontal
        bar.alignment = .center
        bar.spacing = 2
        bar.translatesAutoresizingMaskIntoConstraints = false

        configure(closeButton, symbol: "xmark", label: "閉じる")
        closeButton.addTarget(self, action: #selector(closeTapped), for: .touchUpInside)
        hostLabel.textColor = .white
        hostLabel.font = .boldSystemFont(ofSize: 15)
        // Long hosts keep their registrable end visible.
        hostLabel.lineBreakMode = .byTruncatingHead
        hostLabel.setContentHuggingPriority(.defaultLow, for: .horizontal)
        hostLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        configure(backButton, symbol: "chevron.left", label: "戻る")
        backButton.addTarget(self, action: #selector(backTapped), for: .touchUpInside)
        configure(reloadButton, symbol: "arrow.clockwise", label: "再読み込み")
        reloadButton.addTarget(self, action: #selector(reloadTapped), for: .touchUpInside)
        configure(menuButton, symbol: "ellipsis.circle", label: "メニュー")
        menuButton.showsMenuAsPrimaryAction = true

        for item in [closeButton, hostLabel, backButton, reloadButton, menuButton] as [UIView] {
            bar.addArrangedSubview(item)
        }

        progressView.translatesAutoresizingMaskIntoConstraints = false
        progressView.progressTintColor = UIColor(red: 0xf5 / 255, green: 0x48 / 255, blue: 0x3d / 255, alpha: 1)
        progressView.trackTintColor = .clear

        container.backgroundColor = .white
        container.translatesAutoresizingMaskIntoConstraints = false

        view.addSubview(bar)
        view.addSubview(progressView)
        view.addSubview(container)
        let guide = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            bar.topAnchor.constraint(equalTo: guide.topAnchor),
            bar.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: 4),
            bar.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -4),
            bar.heightAnchor.constraint(equalToConstant: 44),
            progressView.topAnchor.constraint(equalTo: bar.bottomAnchor),
            progressView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            progressView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            progressView.heightAnchor.constraint(equalToConstant: 2),
            container.topAnchor.constraint(equalTo: progressView.bottomAnchor),
            container.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            container.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            container.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
    }

    private func configure(_ button: UIButton, symbol: String, label: String) {
        button.setImage(UIImage(systemName: symbol), for: .normal)
        button.tintColor = .white
        button.accessibilityLabel = label
        button.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            button.widthAnchor.constraint(equalToConstant: 44),
            button.heightAnchor.constraint(equalToConstant: 44),
        ])
    }

    private func makeMenu() -> UIMenu {
        var actions: [UIMenuElement] = [
            UIAction(title: "共有", image: UIImage(systemName: "square.and.arrow.up")) { [weak self] _ in
                self?.share()
            },
            UIAction(title: "ブラウザの画面で開く", image: UIImage(systemName: "rectangle.portrait.on.rectangle.portrait")) { [weak self] _ in
                guard let self = self, let url = self.shareableURL() ?? self.session.launchURL else { return }
                self.presentBrowserView(url)
            },
            UIAction(title: "Safariで開く", image: UIImage(systemName: "safari")) { [weak self] _ in
                guard let url = self?.shareableURL() else { return }
                self?.openExternally(url)
            },
        ]
        if !session.popup, let client = session.client {
            let title = client.verified ? "\(client.name)で開く" : "\(client.name)で開く…"
            actions.append(
                UIAction(title: title, image: UIImage(systemName: "app.badge")) { [weak self] _ in
                    self?.openClient(client)
                })
        }
        if !session.popup {
            actions.append(
                UIAction(title: "開き方を変える", image: UIImage(systemName: "arrow.triangle.branch")) { [weak self] _ in
                    self?.askShell(AppScreenActions.change)
                })
            actions.append(
                UIAction(title: "アプリを切り替える", image: UIImage(systemName: "square.grid.2x2")) { [weak self] _ in
                    self?.switchApps()
                })
        }
        actions.append(
            UIAction(title: "閉じる", image: UIImage(systemName: "xmark"), attributes: .destructive) { [weak self] _ in
                self?.closeScreen()
            })
        return UIMenu(title: "", children: actions)
    }

    func updateBar() {
        guard isViewLoaded else { return }
        let webView = session.webView
        let shown = session.navigator.committedURL ?? session.launchURL
        let host = AppScreenPolicy.displayHost(shown)
        let secure = shown?.scheme?.lowercased() == "https"
        hostLabel.text = secure ? "🔒 \(host)" : host
        hostLabel.accessibilityLabel = "表示中のサイト \(host)"
        backButton.isEnabled = webView.canGoBack
        backButton.alpha = webView.canGoBack ? 1 : 0.35
    }

    func updateProgress() {
        guard isViewLoaded else { return }
        let webView = session.webView
        progressView.isHidden = !webView.isLoading
        progressView.setProgress(Float(webView.estimatedProgress), animated: webView.isLoading)
    }

    @objc private func closeTapped() { closeScreen() }

    @objc private func backTapped() {
        if session.webView.canGoBack { session.webView.goBack() }
    }

    @objc private func reloadTapped() { session.webView.reload() }

    private func currentLaunchableURL() -> URL? {
        guard let url = session.navigator.committedURL,
              AppScreenPolicy.isInScreen(url.absoluteString, origins: session.navigator.origins) else {
            return nil
        }
        return url
    }

    /// 「{name}で開く」: a verified app opens directly; a missing one, or an
    /// unverified one (confirmed with its real destination shown), goes to the
    /// shell's chooser.
    private func openClient(_ client: AppScreenBannerClient) {
        guard client.verified else {
            askShell(AppScreenActions.unverified)
            return
        }
        AppClientOpener.open(universalLink: client.universalLink, scheme: client.open) { [weak self] opened in
            if !opened { self?.askShell(AppScreenActions.notInstalled) }
        }
    }

    /// Leave the chooser for this app to the shell and go back to it; the page
    /// stays alive.
    private func askShell(_ reason: String) {
        guard let url = session.launchURL else { return }
        AppScreenActions.request(AppScreenPendingAction(key: session.key, url: url.absoluteString, reason: reason))
        switchApps()
    }

    /// A main-frame page outside this screen's origins. Opened in the browser
    /// view at once; without a user gesture (a redirect, a script) at most once
    /// every few seconds, so a page cannot keep presenting it.
    func handOffToBrowserView(_ url: URL, userGesture: Bool) {
        if !userGesture, let last = lastBrowserViewAt,
           Date().timeIntervalSince(last) < Self.browserViewCooldown {
            return
        }
        lastBrowserViewAt = Date()
        presentBrowserView(url)
    }

    private func presentBrowserView(_ url: URL) {
        guard viewIfLoaded?.window != nil, !isBeingDismissed else { return }
        BrowserView.present(url, from: self)
    }

    /// A request to another origin that cannot be handed over as a link (a form
    /// POST) was refused; offer to continue from this page in the browser view.
    func offerBrowserViewForPage(blocked url: URL) {
        guard canPresent() else { return }
        if let declined = lastExternalDeclinedAt, Date().timeIntervalSince(declined) < Self.promptCooldown {
            return
        }
        let host = AppScreenPolicy.displayHost(url)
        let alert = UIAlertController(
            title: "ブラウザの画面で続ける",
            message: "\(host.isEmpty ? "ほかのサイト" : host) への送信は Takosumi の中ではできません。このページをブラウザの画面で開いて続けますか？",
            preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "キャンセル", style: .cancel) { [weak self] _ in
            self?.lastExternalDeclinedAt = Date()
        })
        alert.addAction(UIAlertAction(title: "ブラウザの画面で開く", style: .default) { [weak self] _ in
            guard let self = self, let page = self.shareableURL() ?? self.session.launchURL else { return }
            self.presentBrowserView(page)
        })
        present(alert, animated: true)
    }

    /// The current page without a spent OAuth code or a token in its URL, as
    /// it may be shared or handed to the browser.
    private func shareableURL() -> URL? {
        currentLaunchableURL().flatMap { URL(string: AppScreenPolicy.shareable($0.absoluteString)) }
    }

    private func share() {
        guard let url = shareableURL(), presentedViewController == nil else { return }
        let activity = UIActivityViewController(activityItems: [url], applicationActivities: nil)
        activity.popoverPresentationController?.sourceView = menuButton
        activity.popoverPresentationController?.sourceRect = menuButton.bounds
        present(activity, animated: true)
    }

    // MARK: Closing and switching

    /// The popup screens presented above this one, nearest first.
    private func popupChain() -> [AppScreenViewController] {
        var chain: [AppScreenViewController] = []
        var next = presentedViewController
        while let current = next {
            if let screen = current as? AppScreenViewController { chain.append(screen) }
            next = current.presentedViewController
        }
        return chain
    }

    private func removePopupChain() {
        for popupScreen in popupChain() {
            popupScreen.pendingDialog?.cancel()
            coordinator.remove(popupScreen.session.key)
        }
        pendingDialog?.cancel()
    }

    /// Back to the Takosumi home; the page stays alive for the next switch.
    private func switchApps() {
        removePopupChain()
        presentingViewController?.dismiss(animated: true)
    }

    private func closeScreen() {
        let closing = session
        removePopupChain()
        presentingViewController?.dismiss(animated: true) { [weak self, coordinator] in
            // Reopened meanwhile in a new controller: keep it.
            let owner = closing.navigator.screen
            if owner == nil || owner === self { coordinator.remove(closing.key) }
        }
    }

    private func ownsSession() -> Bool {
        let owner = session.navigator.screen
        return owner == nil || owner === self
    }

    func pageClosed(_ webView: WKWebView) {
        if webView === session.webView { closeScreen() }
    }

    // MARK: Prompts and popups

    private func canPresent() -> Bool {
        viewIfLoaded?.window != nil && presentedViewController == nil &&
            !isBeingDismissed && !isBeingPresented && transitionCoordinator == nil
    }

    func presentPopup(configuration: WKWebViewConfiguration, origins: AppScreenOrigins?) -> WKWebView? {
        guard canPresent() else { return nil }
        let popup = coordinator.makePopupSession(configuration: configuration, origins: origins)
        popup.title = session.title
        let controller = AppScreenViewController(coordinator: coordinator, session: popup)
        controller.modalPresentationStyle = .fullScreen
        controller.loadViewIfNeeded()
        present(controller, animated: true)
        guard presentedViewController === controller else {
            coordinator.remove(popup.key)
            return nil
        }
        return popup.webView
    }

    func presentDialog(
        title: String,
        message: String,
        completion: DialogCompletion,
        actions: [(String, UIAlertAction.Style, () -> Void)],
        textField: DialogTextField?
    ) {
        guard canPresent() else {
            completion.cancel()
            return
        }
        let alert = UIAlertController(title: title, message: message, preferredStyle: .alert)
        if let textField = textField {
            alert.addTextField { field in
                field.text = textField.defaultText
                textField.field = field
            }
        }
        for (label, style, handler) in actions {
            alert.addAction(UIAlertAction(title: label, style: style) { _ in completion.finish(handler) })
        }
        pendingDialog = completion
        present(alert, animated: true)
        if presentedViewController !== alert { completion.cancel() }
    }

    /// Ask before leaving the app. Without a user gesture, a page that was just
    /// declined cannot ask again for a few seconds (no prompt loops).
    func confirmExternal(_ url: URL, insecureHttp: Bool, download: Bool = false, userGesture: Bool) {
        guard canPresent() else { return }
        if !userGesture, let declined = lastExternalDeclinedAt,
           Date().timeIntervalSince(declined) < Self.promptCooldown {
            return
        }
        let host = AppScreenPolicy.displayHost(session.navigator.committedURL)
        let origin = host.isEmpty ? "このページ" : host
        let message: String
        if download {
            message = "\(origin) のファイルをSafariで開きますか？\n\n\(url.absoluteString)"
        } else if insecureHttp {
            message = "このページは https ではないため、Takosumi の中では開けません。Safariで開きますか？\n\n\(url.absoluteString)"
        } else {
            message = "\(origin) が外部のアプリを開こうとしています。開きますか？\n\n\(url.absoluteString)"
        }
        let alert = UIAlertController(title: "外部で開く", message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "キャンセル", style: .cancel) { [weak self] _ in
            self?.lastExternalDeclinedAt = Date()
        })
        alert.addAction(UIAlertAction(title: "開く", style: .default) { [weak self] _ in
            self?.openExternally(url)
        })
        present(alert, animated: true)
    }

    private func openExternally(_ url: URL) {
        UIApplication.shared.open(url, options: [:]) { [weak self] opened in
            guard !opened, let self = self, self.canPresent() else { return }
            let alert = UIAlertController(title: nil, message: "開けるアプリがありません", preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "OK", style: .default))
            self.present(alert, animated: true)
        }
    }
}
