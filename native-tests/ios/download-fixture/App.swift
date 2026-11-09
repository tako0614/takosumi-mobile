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
    private var closedDuringTransfer = false
    private var closeTimedOut = false
    private let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    private let sentinel = FileManager.default.temporaryDirectory.appendingPathComponent("fixture-sentinel-\(UUID().uuidString)")
    private let sentinelBytes = Data("unrelated fixture file\n".utf8)
    private let cookieBytes = Data("Takosumi same-profile cookie download\n".utf8)
    private let blobBytes = Data("Takosumi Blob download\n".utf8)

    override func viewDidLoad() {
        super.viewDidLoad()
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
        receipt.text = "not inspected"
        stack.addArrangedSubview(receipt)
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -20),
            stack.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor),
        ])
    }

    private func open(profile: String, closeOnBytes: Bool = false) {
        inspectionTimer?.invalidate()
        guard let raw = ProcessInfo.processInfo.environment["SUMI_DOWNLOAD_URL"],
              let url = URL(string: raw), let key = AppScreenPolicy.webHomeKey(raw, profile: profile) else {
            fatalError("Missing canonical HTTPS fixture URL")
        }
        var request = AppScreenRequest(key: key, url: url, title: "Download Fixture")
        request.webHome = true
        request.profile = profile
        do { try AppScreenCoordinator.shared.open(request, from: self) }
        catch { fatalError("Actual AppScreen open failed: \(error)") }
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
        ]
        do {
            let bytes = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
            // This receipt contains observations only, never replacement files.
            try bytes.write(to: documents.appendingPathComponent("fixture-receipt.json"), options: .atomic)
            receipt.text = String(data: bytes, encoding: .utf8)
        } catch { receipt.text = "receipt write failed" }
    }
}
