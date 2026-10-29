import UIKit
import WebKit

struct AppScreenRequest {
    let key: String
    let url: URL
    let title: String
}

final class AppScreenSession {
    let key: String
    let webView: WKWebView
    let popup: Bool
    let navigator: AppScreenNavigator
    /// URL the screen was opened with; reopening with it keeps the page state.
    var launchURL: URL?
    var title: String = ""

    init(key: String, webView: WKWebView, popup: Bool) {
        self.key = key
        self.webView = webView
        self.popup = popup
        navigator = AppScreenNavigator(webView: webView)
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
                self?.refreshCommitted(webView)
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

    /// While a load is in flight, `url` may already be the next page's.
    private func refreshCommitted(_ webView: WKWebView) {
        if !webView.isLoading {
            committedURL = webView.url
        }
        screen?.updateBar()
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
        switch AppScreenPolicy.decide(url?.absoluteString ?? "", mainFrame: mainFrame) {
        case .inApp:
            decisionHandler(.allow)
        case .block:
            decisionHandler(.cancel)
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
            screen?.confirmExternal(url, insecureHttp: false, download: true, userGesture: true)
            return
        }
        decisionHandler(.allow)
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        committedURL = webView.url
        screen?.updateBar()
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        if !webView.isLoading { committedURL = webView.url }
        screen?.updateBar()
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
        screen?.presentPopup(configuration: configuration)
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

/// Keeps app screen webviews alive across switches (at most `maxAlive`, least
/// recently used closed first) and presents them over the Tauri shell.
///
/// Every screen uses the default website data store, so cookies and storage are
/// one profile across apps: a Takosumi sign-in made in one app screen carries
/// over to the others. Do not give screens their own data store.
final class AppScreenCoordinator {
    static let shared = AppScreenCoordinator()
    static let maxAlive = 4

    private let processPool = WKProcessPool()
    private var sessions: [String: AppScreenSession] = [:]
    private var order: [String] = []
    private var visibleKeys: Set<String> = []
    private weak var screen: AppScreenViewController?

    func open(_ request: AppScreenRequest, from presenter: UIViewController) {
        let existing = sessions[request.key]
        let session = existing ?? makeSession(key: request.key, popup: false, configuration: nil)
        if existing == nil { session.launchURL = request.url }
        session.title = request.title
        touch(session.key)
        let keep = existing != nil && existing?.webView.url != nil && existing?.launchURL == request.url
        if !keep { session.webView.load(URLRequest(url: request.url)) }

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

    func makePopupSession(configuration: WKWebViewConfiguration) -> AppScreenSession {
        let session = makeSession(key: "popup:\(UUID().uuidString)", popup: true, configuration: configuration)
        evict(keeping: session.key)
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
        // TASK-0119 U4 step 4 adds the document-start Push API bridge here.
        return configuration
    }

    private func touch(_ key: String) {
        order.removeAll { $0 == key }
        order.append(key)
    }

    private func evict(keeping key: String) {
        while sessions.count > Self.maxAlive {
            guard let victim = order.first(where: { $0 != key && !visibleKeys.contains($0) }) else {
                return
            }
            remove(victim)
        }
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
/// share, open in Safari and switch apps.
final class AppScreenViewController: UIViewController {
    private static let barColor = UIColor(red: 0x15 / 255, green: 0x15 / 255, blue: 0x17 / 255, alpha: 1)
    private static let promptCooldown: TimeInterval = 5

    private let coordinator: AppScreenCoordinator
    private(set) var session: AppScreenSession
    private var attachedSession: AppScreenSession?
    private var pendingDialog: DialogCompletion?
    private var lastExternalDeclinedAt: Date?

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
        if isViewLoaded { attach(next) }
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
            UIAction(title: "Safariで開く", image: UIImage(systemName: "safari")) { [weak self] _ in
                guard let url = self?.currentLaunchableURL() else { return }
                self?.openExternally(url)
            },
        ]
        if !session.popup {
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
              AppScreenPolicy.isLaunchable(url.absoluteString) else {
            return nil
        }
        return url
    }

    private func share() {
        guard let url = currentLaunchableURL(), presentedViewController == nil else { return }
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

    func presentPopup(configuration: WKWebViewConfiguration) -> WKWebView? {
        guard canPresent() else { return nil }
        let popup = coordinator.makePopupSession(configuration: configuration)
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
