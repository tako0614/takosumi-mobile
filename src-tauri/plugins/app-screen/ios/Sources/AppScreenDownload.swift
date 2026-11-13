import UIKit
import WebKit

/// The session retains this interface even on OS versions without WKDownload.
@MainActor
protocol AppScreenDownloadActivity: AnyObject {
    var id: UUID { get }
    func cancel()
    func cancelUnconfirmed()
    func resumePresentation()
}

/// Receives the original WebKit response, never a reissued URLRequest or an
/// exported cookie. There is one activity per session, including the OS export.
@available(iOS 14.5, *)
@MainActor
final class AppScreenDownload: NSObject, AppScreenDownloadActivity, WKDownloadDelegate {
    let id: UUID
    private enum State: String { case waiting, confirming, receiving, ready, exporting, failed, cancelling, finished }
    private var state = State.waiting {
        didSet { observeLifecycle("state") }
    }
#if SUMI_DOWNLOAD_FIXTURE
    // The standalone fixture observes closed event/state names only. It never
    // receives or retains this owner, a controller, a URL or response data.
    static var lifecycleObserver: ((String, String) -> Void)?
#endif
    private let download: WKDownload
    private let sourceOrigin: String
    private let context: () -> UIViewController?
    private let declined: () -> Void
    private let ended: () -> Void
    private var destinationReply: ((URL?) -> Void)?
    private var directory: URL?
    private var file: URL?
    private var modal: UIViewController?
    private var backgroundObserver: NSObjectProtocol?
    private var activeObserver: NSObjectProtocol?
    private var progressObservation: NSKeyValueObservation?

    init(id: UUID, download: WKDownload, sourceOrigin: String,
         context: @escaping () -> UIViewController?, declined: @escaping () -> Void,
         ended: @escaping () -> Void) {
        self.id = id
        self.download = download
        self.sourceOrigin = sourceOrigin
        self.context = context
        self.declined = declined
        self.ended = ended
        super.init()
        observeLifecycle("created")
        backgroundObserver = NotificationCenter.default.addObserver(
            forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main
        ) { [weak self] _ in
            // An OS export may hand control to a file provider. Its completion
            // owns the file lifetime; backgrounding must not remove it early.
            Task { @MainActor [weak self] in
                guard let self = self, self.state != .exporting else { return }
                self.cancel()
            }
        }
        activeObserver = NotificationCenter.default.addObserver(
            forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in self?.resumePresentation() }
        }
    }

    /// The navigator must retain the owner before WebKit can call its delegate.
    func start() { download.delegate = self }

    func download(_ download: WKDownload, decideDestinationUsing response: URLResponse,
                  suggestedFilename: String, completionHandler: @escaping (URL?) -> Void) {
        guard download === self.download, state == .waiting,
              AppScreenPolicy.isDownloadTarget(response.url?.absoluteString ?? "", sourceOrigin: sourceOrigin),
              (response as? HTTPURLResponse).map({ (200..<300).contains($0.statusCode) }) ?? true,
              context() != nil else {
            completionHandler(nil)
            cancel()
            return
        }
        destinationReply = completionHandler
        state = .confirming
        let name = AppScreenPolicy.downloadFilename(suggestedFilename)
        let alert = UIAlertController(title: "ファイルを保存",
                                      message: "\(sourceOrigin)\n\n\(name)", preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "キャンセル", style: .cancel) { [weak self] _ in self?.cancelFromUI() })
        alert.addAction(UIAlertAction(title: "保存", style: .default) { [weak self] _ in
            self?.dismissModal { [weak self] in self?.receive(name: name) }
        })
        if !present(alert) { cancel() }
    }

    private func receive(name: String) {
        guard state == .confirming, context() != nil else { cancel(); return }
        do {
            let root = FileManager.default.temporaryDirectory.appendingPathComponent(
                "takosumi-download-\(UUID().uuidString)", isDirectory: true)
            // A fresh directory belongs only to this transfer. Never overwrite
            // a suggested path, or remove any parent/shared temporary content.
            guard !FileManager.default.fileExists(atPath: root.path) else { stop(reportFailure: true); return }
            try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false,
                                                    attributes: [.posixPermissions: 0o700])
            directory = root
            let target = root.appendingPathComponent(name, isDirectory: false)
            guard !FileManager.default.fileExists(atPath: target.path) else { stop(reportFailure: true); return }
            file = target
            state = .receiving
            let progress = UIAlertController(title: "ダウンロード中", message: name, preferredStyle: .alert)
            progress.addAction(UIAlertAction(title: "キャンセル", style: .cancel) { [weak self] _ in self?.cancelFromUI() })
            guard present(progress) else { cancel(); return }
            progressObservation = download.progress.observe(\.completedUnitCount, options: [.new]) { [weak self] _, _ in
                Task { @MainActor [weak self] in self?.updateProgress() }
            }
            reply(with: target)
        } catch {
            stop(reportFailure: true)
        }
    }

    private func updateProgress() {
        guard state == .receiving, let alert = modal as? UIAlertController, let file = file else { return }
        let progress = download.progress
        let received = ByteCountFormatter.string(fromByteCount: max(0, progress.completedUnitCount), countStyle: .file)
        let total = progress.totalUnitCount > 0
            ? " / " + ByteCountFormatter.string(fromByteCount: progress.totalUnitCount, countStyle: .file) : ""
        alert.message = "\(file.lastPathComponent)\n\n\(received)\(total)"
    }

    func download(_ download: WKDownload, willPerformHTTPRedirection response: HTTPURLResponse,
                  newRequest request: URLRequest, decisionHandler: @escaping (WKDownload.RedirectPolicy) -> Void) {
        let allowed = download === self.download && state != .cancelling && state != .finished &&
            AppScreenPolicy.isLaunchable(request.url?.absoluteString ?? "")
        decisionHandler(allowed ? .allow : .cancel)
    }

    func download(_ download: WKDownload, didReceive challenge: URLAuthenticationChallenge,
                  completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        // Keep system TLS validation. Never supply another profile's HTTP auth
        // credentials or a trust exception from a process-wide credential store.
        let trust = download === self.download && state != .cancelling && state != .finished &&
            challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust
        completionHandler(trust ? .performDefaultHandling : .cancelAuthenticationChallenge, nil)
    }

    func downloadDidFinish(_ download: WKDownload) {
        guard download === self.download, state == .receiving, let file = file else { return }
        do {
            let values = try file.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
            guard values.isRegularFile == true, values.isSymbolicLink != true else { fail(); return }
        } catch { fail(); return }
        state = .ready
        progressObservation = nil
        dismissModal { [weak self] in self?.resumePresentation() }
    }

    func download(_ download: WKDownload, didFailWithError error: Error, resumeData: Data?) {
        if download === self.download { observeLifecycle("download-failed") }
        guard download === self.download, state != .cancelling, state != .finished else { return }
        fail()
    }

    private func fail() {
        reply(with: nil)
        state = .failed
        progressObservation = nil
        dismissModal { [weak self] in
            guard let self = self, self.state == .failed else { return }
            self.observeLifecycle("failure-cleanup")
            self.removeTemporaryFile()
            let alert = UIAlertController(title: "保存できませんでした",
                                          message: "ページからもう一度ダウンロードしてください。", preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "OK", style: .default) { [weak self] _ in self?.finish() })
            if !self.present(alert) { self.finish() }
        }
    }

    func resumePresentation() {
        guard state == .ready, let file = file, let presenter = context(),
              presenter.viewIfLoaded?.window != nil, !presenter.isBeingDismissed,
              presenter.presentedViewController == nil, presenter.transitionCoordinator == nil else { return }
        let activity = UIActivityViewController(activityItems: [file], applicationActivities: nil)
        activity.popoverPresentationController?.sourceView = presenter.view
        activity.popoverPresentationController?.sourceRect = CGRect(
            x: presenter.view.bounds.midX, y: presenter.view.bounds.midY, width: 1, height: 1)
        // The navigator can release us when its session closes. An OS service
        // may still be reading this URL, so its completion owns both us and the
        // file. A view-controller dismissal completion is not that boundary.
        activity.completionWithItemsHandler = { [self, weak activity] _, _, _, _ in
            observeLifecycle("activity-completed")
            activity?.completionWithItemsHandler = nil
            finish()
        }
        state = .exporting
        modal = activity
        // UIKit may forward or finish this presentation after present returns.
        // An immediate presenter/controller comparison cannot revoke its file
        // ownership: session removal could then unlink the OS service's input.
        // From this handoff onward only the activity completion releases it.
        observeLifecycle("export-present-requested")
        presenter.present(activity, animated: false) { [weak self] in
            // Presentation completion is diagnostic, not export completion.
            self?.observeLifecycle("export-present-completed")
        }
    }

    func cancelUnconfirmed() {
        if state == .waiting || state == .confirming { cancel() }
    }

    func cancel() {
        stop(reportFailure: false)
    }

    private func cancelFromUI() {
        declined()
        cancel()
    }

    private func stop(reportFailure: Bool) {
        observeLifecycle("stop")
        // After handing the file to the OS, session changes stop owning this
        // flow but must not dismiss its activity or unlink a provider's input.
        // The activity's own completion releases it, even if the session is gone.
        guard state != .exporting, state != .cancelling, state != .finished else { return }
        let alreadyStopped = state == .ready || state == .failed
        state = .cancelling
        progressObservation = nil
        reply(with: nil)
        let stopped = { [self] in
            if reportFailure { fail() } else { finish() }
        }
        dismissModal { [self] in
            if alreadyStopped {
                stopped()
            } else {
                // Wait until WebKit stops writing before unlinking the partial.
                // This callback also keeps the owner alive after session removal.
                download.cancel { _ in stopped() }
            }
        }
    }

    private func reply(with url: URL?) {
        let reply = destinationReply
        destinationReply = nil
        reply?(url)
    }

    private func present(_ controller: UIViewController) -> Bool {
        guard let presenter = context(), presenter.presentedViewController == nil,
              !presenter.isBeingDismissed, presenter.transitionCoordinator == nil else {
            observeLifecycle("present-refused")
            return false
        }
        modal = controller
        // Complete presentation before a tiny download can synchronously finish
        // and replace this alert with its export controller.
        presenter.present(controller, animated: false)
        if presenter.presentedViewController !== controller {
            observeLifecycle("present-unconfirmed")
            modal = nil
            return false
        }
        observeLifecycle("present-confirmed")
        return true
    }

    private func dismissModal(_ completion: @escaping () -> Void) {
        let owned = modal
        modal = nil
        if let owned = owned, owned.presentingViewController != nil {
            owned.dismiss(animated: false, completion: completion)
        } else { completion() }
    }

    private func removeTemporaryFile() {
        guard let directory = directory else { return }
        observeLifecycle("remove-started")
        do {
            try FileManager.default.removeItem(at: directory)
            observeLifecycle("remove-succeeded")
            self.directory = nil
            file = nil
        } catch {
            observeLifecycle("remove-failed")
            // No path, response URL, suggested filename or credential in logs.
            NSLog("AppScreen download temporary cleanup failed")
        }
    }

    private func finish() {
        observeLifecycle("finish")
        guard state != .finished else { return }
        state = .finished
        reply(with: nil)
        download.delegate = nil
        if let observer = backgroundObserver { NotificationCenter.default.removeObserver(observer) }
        if let observer = activeObserver { NotificationCenter.default.removeObserver(observer) }
        backgroundObserver = nil
        activeObserver = nil
        progressObservation = nil
        modal = nil
        removeTemporaryFile()
        ended()
    }

    private func observeLifecycle(_ event: String) {
#if SUMI_DOWNLOAD_FIXTURE
        Self.lifecycleObserver?(event, state.rawValue)
#endif
    }
}
