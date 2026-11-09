import UIKit

// Only the unused shell signal is replaced. The coordinator, navigator,
// WebView, policy and WKDownload owner compile from the product source files.
final class AppScreenPlugin {
    func notifyActionPending() { fatalError("The fixture must not enter the legacy shell chooser") }
}

@main
final class DownloadFixtureApp: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(_ application: UIApplication,
                     didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]?) -> Bool {
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = FixtureHome()
        window.makeKeyAndVisible()
        self.window = window
        return true
    }
}

final class FixtureHome: UIViewController {
    private let work = UUID().uuidString.lowercased()
    private let personal = UUID().uuidString.lowercased()
    private let receipt = UILabel()
    private var closeTimer: Timer?
    private var inspectionTimer: Timer?
    private var exportObservationTimer: Timer?
    private var exportObservations: [[String: Any]] = []
    private var downloadLifecycle: [[String: Any]] = []
    private var lifecycleTruncated = false
    private var closedDuringTransfer = false
    private var closeTimedOut = false
    private var closedDuringExport = false
    private var exportFileRetained = false
    private var exportCloseTimedOut = false
    private let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    private let sentinel = FileManager.default.temporaryDirectory.appendingPathComponent("fixture-sentinel-\(UUID().uuidString)")
    private let sentinelBytes = Data("unrelated fixture file\n".utf8)
    private let cookieBytes = Data("Takosumi same-profile cookie download\n".utf8)
    private let blobBytes = Data("Takosumi Blob download\n".utf8)

    override func viewDidLoad() {
        super.viewDidLoad()
        AppScreenDownload.lifecycleObserver = { [weak self] event, state in
            guard let self = self else { return }
            if self.downloadLifecycle.count < 64 {
                self.downloadLifecycle.append([
                    "unixSeconds": Date().timeIntervalSince1970,
                    "event": event,
                    "state": state,
                ])
            } else { self.lifecycleTruncated = true }
        }
        view.backgroundColor = .systemBackground
        do {
            try FileManager.default.createDirectory(at: documents, withIntermediateDirectories: true)
            // Make this disposable app's Documents visible in On My iPhone.
            // The host NEVER writes cookie.txt or blob.txt; only OS export does.
            try sentinelBytes.write(to: documents.appendingPathComponent("keep.txt"), options: .withoutOverwriting)
            try sentinelBytes.write(to: sentinel, options: .withoutOverwriting)
        } catch { fatalError("Fixture setup failed: \(error)") }
        let stack = UIStackView()
        stack.axis = .vertical
        stack.spacing = 20
        stack.translatesAutoresizingMaskIntoConstraints = false
        for (name, action) in [
            ("Work", #selector(openWork)),
            ("Personal", #selector(openPersonal)),
            ("Work, close during export", #selector(openAndCloseExport)),
            ("Work, close during transfer", #selector(openAndClose)),
            ("Inspect files", #selector(inspectFiles)),
        ] {
            let button = UIButton(type: .system)
            button.setTitle(name, for: .normal)
            button.addTarget(self, action: action, for: .touchUpInside)
            stack.addArrangedSubview(button)
        }
        receipt.numberOfLines = 0
        receipt.accessibilityIdentifier = "fixture-receipt"
        // Read the built bundle, not the project settings: Files must see this
        // disposable app's actual Documents-sharing flags and seeded folder.
        let configuration: [String: Any] = [
            "fileSharing": Bundle.main.object(forInfoDictionaryKey: "UIFileSharingEnabled") as? Bool ?? false,
            "documentsInPlace": Bundle.main.object(forInfoDictionaryKey: "LSSupportsOpeningDocumentsInPlace") as? Bool ?? false,
            "displayName": Bundle.main.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String ?? "",
            "seedPresent": (try? Data(contentsOf: documents.appendingPathComponent("keep.txt"))) == sentinelBytes,
        ]
        receipt.text = String(data: try! JSONSerialization.data(withJSONObject: configuration, options: .sortedKeys), encoding: .utf8)
        stack.addArrangedSubview(receipt)
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -20),
            stack.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor),
        ])
    }

    private func open(profile: String, closeOnBytes: Bool = false, closeOnExport: Bool = false) {
        inspectionTimer?.invalidate()
        closeTimer?.invalidate()
        guard let raw = ProcessInfo.processInfo.environment["SUMI_DOWNLOAD_URL"],
              let url = URL(string: raw), let key = AppScreenPolicy.webHomeKey(raw, profile: profile) else {
            fatalError("Missing canonical HTTPS fixture URL")
        }
        var request = AppScreenRequest(key: key, url: url, title: "Download Fixture")
        request.webHome = true
        request.profile = profile
        do { try AppScreenCoordinator.shared.open(request, from: self) }
        catch { fatalError("Actual AppScreen open failed: \(error)") }
        if closeOnExport { closeDuringExport(key: key) }
        if closeOnBytes {
            let deadline = Date().addingTimeInterval(25)
            closeTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] timer in
                guard let self = self else { timer.invalidate(); return }
                let partial = self.temporaryDownloads().contains { directory in
                    ((try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: [.fileSizeKey])) ?? [])
                        .contains { ((try? $0.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0) > 0 }
                }
                if partial {
                    timer.invalidate()
                    self.closedDuringTransfer = true
                    // Exercise the real owner removal while WebKit is writing.
                    AppScreenCoordinator.shared.remove(key)
                    self.dismiss(animated: false)
                } else if Date() >= deadline {
                    timer.invalidate()
                    self.closeTimedOut = true
                }
            }
        }
    }

    @objc private func openWork() { open(profile: work) }
    @objc private func openPersonal() { open(profile: personal) }
    @objc private func openAndClose() { open(profile: work, closeOnBytes: true) }
    @objc private func openAndCloseExport() { open(profile: work, closeOnExport: true) }

    private func closeDuringExport(key: String) {
        let deadline = Date().addingTimeInterval(60)
        var closedAt: Date?
        var source: URL?
        // Observing retention must not keep UIKit's activity alive for it.
        weak var activity: UIActivityViewController?
        closeTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] timer in
            guard let self = self else { timer.invalidate(); return }
            if let closedAt = closedAt {
                if Date().timeIntervalSince(closedAt) >= 0.5 {
                    // Observe the real product file after removal/dismissal
                    // callbacks had time to run, before OS export is selected.
                    self.exportFileRetained = source.flatMap { try? Data(contentsOf: $0) } == self.cookieBytes &&
                        activity?.presentingViewController != nil
                    if let source = source, let activity = activity {
                        self.observePendingExport(source: source, activity: activity)
                    }
                    timer.invalidate()
                    self.closeTimer = nil
                }
            } else {
                var top: UIViewController = self
                while let presented = top.presentedViewController { top = presented }
                if let export = top as? UIActivityViewController {
                    let files = self.temporaryDownloads().map { $0.appendingPathComponent("cookie.txt") }
                        .filter { (try? Data(contentsOf: $0)) == self.cookieBytes }
                    guard files.count == 1 else { self.exportCloseTimedOut = true; timer.invalidate(); return }
                    source = files[0]
                    activity = export
                    self.closedDuringExport = true
                    // No substitute exporter or completion callback. Remove
                    // the actual session while UIKit still owns its activity.
                    AppScreenCoordinator.shared.remove(key)
                    closedAt = Date()
                } else if Date() >= deadline {
                    self.exportCloseTimedOut = true
                    timer.invalidate()
                }
            }
        }
    }

    private func observePendingExport(source: URL, activity: UIActivityViewController) {
        // Passive diagnostics for the same original file after session removal.
        // Neither this timer nor its samples retain UIKit's export controller
        // or the product download owner. Never create a replacement export.
        exportObservationTimer?.invalidate()
        exportObservations = []
        guard samplePendingExport(source: source, activity: activity) else { return }
        exportObservationTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self, weak activity] timer in
            guard let self = self else { timer.invalidate(); return }
            if !self.samplePendingExport(source: source, activity: activity) { timer.invalidate() }
        }
    }

    private func samplePendingExport(source: URL, activity: UIActivityViewController?) -> Bool {
        let presented = activity?.presentingViewController != nil
        let exists = FileManager.default.fileExists(atPath: source.path)
        let bytes = try? Data(contentsOf: source)
        exportObservations.append([
            "unixSeconds": Date().timeIntervalSince1970,
            "sourceExists": exists,
            "sourceReadable": bytes != nil,
            "sourceMatchesOriginal": bytes == cookieBytes,
            "activityAlive": activity != nil,
            "activityPresented": presented,
        ])
        saveExportObservations()
        return presented && exportObservations.count < 32
    }

    private func saveExportObservations() {
        guard !exportObservations.isEmpty else { return }
        let value: [String: Any] = ["diagnosticOnly": true, "samples": exportObservations,
                                  "lifecycle": downloadLifecycle, "lifecycleTruncated": lifecycleTruncated]
        // No file bytes, URLs, paths or credentials enter this receipt.
        // Failure to save diagnostics does not alter any export assertion.
        if let bytes = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) {
            try? bytes.write(to: documents.appendingPathComponent("fixture-export-observations.json"), options: .atomic)
        }
    }

    private func temporaryDownloads() -> [URL] {
        ((try? FileManager.default.contentsOfDirectory(at: FileManager.default.temporaryDirectory,
                                                       includingPropertiesForKeys: nil)) ?? [])
            .filter { $0.lastPathComponent.hasPrefix("takosumi-download-") }
    }

    @objc private func inspectFiles() {
        inspectionTimer?.invalidate()
        updateReceipt()
        let deadline = Date().addingTimeInterval(10)
        inspectionTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] timer in
            guard let self = self else { timer.invalidate(); return }
            self.updateReceipt()
            if self.temporaryDownloads().isEmpty || Date() >= deadline { timer.invalidate() }
        }
    }

    private func updateReceipt() {
        let result: [String: Any] = [
            "cookie": (try? Data(contentsOf: documents.appendingPathComponent("cookie.txt"))) == cookieBytes,
            "blob": (try? Data(contentsOf: documents.appendingPathComponent("blob.txt"))) == blobBytes,
            "temporaryCount": temporaryDownloads().count,
            "sentinel": (try? Data(contentsOf: sentinel)) == sentinelBytes &&
                (try? Data(contentsOf: documents.appendingPathComponent("keep.txt"))) == sentinelBytes,
            "closedDuringTransfer": closedDuringTransfer,
            "closeTimedOut": closeTimedOut,
            "closedDuringExport": closedDuringExport,
            "exportFileRetained": exportFileRetained,
            "exportCloseTimedOut": exportCloseTimedOut,
        ]
        do {
            let bytes = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
            // This receipt contains observations only, never replacement files.
            try bytes.write(to: documents.appendingPathComponent("fixture-receipt.json"), options: .atomic)
            receipt.text = String(data: bytes, encoding: .utf8)
        } catch { receipt.text = "receipt write failed" }
    }
}
