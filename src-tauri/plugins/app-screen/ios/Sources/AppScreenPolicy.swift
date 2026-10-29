import Foundation

/// Mirror of `src/url-policy.ts`. The shared table is
/// `navigation-policy-cases.json`; keep the three implementations equal.
enum AppScreenNavigation: Equatable {
    case inApp
    case externalConfirm(insecureHttp: Bool)
    case block
}

enum AppScreenPolicy {
    private static let shellScheme = "takosumi"
    private static let inScreenAbout: Set<String> = ["about:blank", "about:srcdoc"]
    private static let localContentSchemes: Set<String> = [
        "about", "blob", "chrome", "chrome-extension", "content", "data", "file",
        "filesystem", "javascript", "view-source", "ws", "wss",
    ]
    private static let subframeWebSchemes: Set<String> = [
        "about", "blob", "data", "http", "https", "javascript",
    ]

    static func decide(_ rawUrl: String, mainFrame: Bool) -> AppScreenNavigation {
        let trimmed = rawUrl.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: trimmed),
              let scheme = url.scheme?.lowercased(),
              !scheme.isEmpty else {
            return .block
        }
        if !mainFrame {
            return subframeWebSchemes.contains(scheme) ? .inApp : .block
        }
        if scheme == "https" {
            if hasUserInfo(url) { return .block }
            guard let host = url.host, !host.isEmpty else { return .block }
            return .inApp
        }
        if scheme == "about" && inScreenAbout.contains(trimmed.lowercased()) {
            return .inApp
        }
        if scheme == shellScheme || localContentSchemes.contains(scheme) {
            return .block
        }
        if scheme == "intent", let target = intentTargetScheme(trimmed),
           target == shellScheme || localContentSchemes.contains(target) {
            // An Android intent: URI opens the scheme named in its fragment.
            return .block
        }
        if scheme == "http" {
            if hasUserInfo(url) { return .block }
            return .externalConfirm(insecureHttp: true)
        }
        return .externalConfirm(insecureHttp: false)
    }

    /// The `scheme=` of `intent://...#Intent;scheme=x;end`, lowercased.
    static func intentTargetScheme(_ rawUrl: String) -> String? {
        guard let marker = rawUrl.range(of: "#Intent;") else { return nil }
        for part in rawUrl[marker.upperBound...].split(separator: ";") {
            if part.lowercased().hasPrefix("scheme=") {
                return part.dropFirst("scheme=".count)
                    .trimmingCharacters(in: .whitespaces)
                    .lowercased()
            }
        }
        return nil
    }

    private static func hasUserInfo(_ url: URL) -> Bool {
        !(url.user ?? "").isEmpty || !(url.password ?? "").isEmpty
    }

    /// https with a host and no user info: the only URLs a screen opens with.
    static func isLaunchable(_ rawUrl: String) -> Bool {
        guard let url = URL(string: rawUrl.trimmingCharacters(in: .whitespacesAndNewlines)),
              url.scheme?.lowercased() == "https" else {
            return false
        }
        return decide(rawUrl, mainFrame: true) == .inApp
    }

    /// The host (with port) shown in the top bar.
    static func displayHost(_ url: URL?) -> String {
        guard let url = url else { return "" }
        guard let host = url.host, !host.isEmpty else {
            return url.scheme == "about" ? url.absoluteString : ""
        }
        if let port = url.port { return "\(host):\(port)" }
        return host
    }
}
