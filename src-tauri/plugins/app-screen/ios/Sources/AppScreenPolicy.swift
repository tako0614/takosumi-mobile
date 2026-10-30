import Foundation

/// Mirror of `src/url-policy.ts`. The shared tables are
/// `navigation-policy-cases.json` and `resume-url-cases.json`; keep the three
/// implementations equal (`ios/Tests/PolicyTests` runs this file against
/// them). `.browserView`: an https page the screen does not show (another
/// site such as an external sign-in, or a Takosumi page outside sign-in: the
/// dashboard, account pages, `/api/v1`) opens in `SFSafariViewController`.
enum AppScreenNavigation: Equatable {
    case inApp
    case browserView
    case externalConfirm(insecureHttp: Bool)
    case block
}

/// What one app screen may show: its app's origin in full, and on the
/// Takosumi origins (the instance's Accounts and the instance itself) only
/// `AppScreenPolicy.accountsSignInPaths`.
struct AppScreenOrigins: Equatable {
    let appOrigin: String?
    let accountsOrigins: Set<String>
}

enum AppScreenPolicy {
    private static let shellScheme = "takosumi"
    /// Hosts the Tauri shell serves its own UI and IPC from (and any `*.localhost`).
    static let shellHosts: Set<String> = ["tauri.localhost", "ipc.localhost", "asset.localhost"]
    /// The pages an app's Takosumi sign-in passes through on the Takosumi
    /// origins, compared exactly on the percent-encoded path.
    static let accountsSignInPaths: Set<String> = [
        "/oauth/authorize", "/oauth/upstream/authorize", "/oauth/upstream/callback",
        "/sign-in", "/sign-in/callback",
    ]
    private static let inScreenAbout: Set<String> = ["about:blank", "about:srcdoc"]
    private static let localContentSchemes: Set<String> = [
        "about", "blob", "chrome", "chrome-extension", "content", "data", "file",
        "filesystem", "javascript", "view-source", "ws", "wss",
    ]
    private static let subframeWebSchemes: Set<String> = [
        "about", "blob", "data", "http", "https", "javascript",
    ]

    /// Takosumi's `hasCredentialQueryParams` names plus OAuth/SAML callback values.
    static let credentialQueryNames: Set<String> = [
        "accesskey", "accesstoken", "apikey", "auth", "authorization", "bearertoken",
        "clientsecret", "cookie", "credential", "credentials", "credentialvalue", "jwt",
        "passwd", "password", "privatekey", "proxyauthorization", "refreshtoken", "secret",
        "session", "sessionid", "sessiontoken", "sig", "signature", "signingkey", "token",
        "xamzcredential", "xamzsecuritytoken", "xamzsignature", "xgoogcredential",
        "xgoogsignature", "xapikey", "code", "idtoken", "oauthtoken", "oauthverifier",
        "samlresponse", "ticket",
    ]

    /// Mirror of takosumi `INTERFACE_CLIENT_BLOCKED_SCHEMES`.
    static let blockedClientSchemes: Set<String> = [
        "http", "https", "javascript", "vbscript", "data", "file", "filesystem", "blob",
        "about", "content", "intent", "android-app", "takosumi", "tel", "callto", "sms",
        "smsto", "mms", "mmsto", "mailto", "facetime", "facetime-audio", "facetime-prompt",
        "facetime-audio-prompt", "itms", "itmss", "itms-apps", "itms-appss", "itms-services",
        "itms-beta", "market", "app-settings", "app-prefs", "prefs", "shortcuts",
        "x-web-search", "x-safari-http", "x-safari-https", "googlechrome", "googlechromes",
        "googlechrome-x-callback", "firefox", "firefox-focus", "focus", "microsoft-edge",
        "microsoft-edge-http", "microsoft-edge-https", "opera", "opera-http", "opera-https",
        "touch-http", "touch-https", "brave", "duckduckgo", "ddgquicklink", "samsunginternet",
        "yandexbrowser-open-url", "firefox-klar", "onionhttp", "onionhttps", "ws", "wss", "ftp",
    ]

    static let resumeURLMaxLength = 4_096
    private static let clientLinkMaxLength = 2_048

    static func decide(
        _ rawUrl: String,
        mainFrame: Bool,
        origins: AppScreenOrigins? = nil
    ) -> AppScreenNavigation {
        let trimmed = rawUrl.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: trimmed),
              let scheme = url.scheme?.lowercased(),
              !scheme.isEmpty else {
            return .block
        }
        let web = scheme == "http" || scheme == "https"
        // The shell's own hosts, in every frame and both web schemes.
        if web, let host = url.host, isShellHost(host) { return .block }
        if !mainFrame {
            guard subframeWebSchemes.contains(scheme) else { return .block }
            if web, let origins = origins,
               isTakosumiPageOutsideSignIn(origin(of: trimmed), encodedPath(trimmed), origins) {
                return .block
            }
            return .inApp
        }
        if scheme == "https" {
            if hasUserInfo(url) || hasRawUserInfo(trimmed) { return .block }
            guard let host = url.host, !host.isEmpty else { return .block }
            if let origins = origins {
                return mainFrameDecision(origin(of: trimmed), encodedPath(trimmed), origins)
            }
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
            if hasUserInfo(url) || hasRawUserInfo(trimmed) { return .block }
            return .externalConfirm(insecureHttp: true)
        }
        return .externalConfirm(insecureHttp: false)
    }

    /// The `scheme=` of `intent://...#Intent;scheme=x;end`, lowercased.
    static func intentTargetScheme(_ rawUrl: String) -> String? {
        // Like Android's Intent.parseUri: the parameters follow the last '#'.
        guard let hash = rawUrl.lastIndex(of: "#") else { return nil }
        let parameters = rawUrl[rawUrl.index(after: hash)...]
        guard parameters.hasPrefix("Intent;") else { return nil }
        for part in parameters.dropFirst("Intent;".count).split(separator: ";") {
            if part.lowercased().hasPrefix("scheme=") {
                return part.dropFirst("scheme=".count)
                    .trimmingCharacters(in: .whitespaces)
                    .lowercased()
            }
        }
        // Without scheme=, `intent:x://...` opens the data part's own scheme.
        guard let colon = rawUrl.firstIndex(of: ":"), colon < hash else { return nil }
        let data = rawUrl[rawUrl.index(after: colon)..<hash]
        guard let end = data.firstIndex(of: ":") else { return nil }
        let scheme = data[data.startIndex..<end]
        guard let first = scheme.first, first.isLetter,
              scheme.allSatisfy({ $0.isLetter || $0.isNumber || "+.-".contains($0) }) else {
            return nil
        }
        return scheme.lowercased()
    }

    private static func hasUserInfo(_ url: URL) -> Bool {
        !(url.user ?? "").isEmpty || !(url.password ?? "").isEmpty
    }

    /// An `@` in the authority, even with empty user info (`https://:@host/`).
    static func hasRawUserInfo(_ rawUrl: String) -> Bool {
        guard let separator = rawUrl.range(of: "://") else { return false }
        return rawUrl[separator.upperBound...].prefix { !"/?#\\".contains($0) }.contains("@")
    }

    static func isShellHost(_ host: String) -> Bool {
        var normalized = host.lowercased()
        while normalized.hasSuffix(".") { normalized.removeLast() }
        return shellHosts.contains(normalized) || normalized.hasSuffix(".localhost")
    }

    /// An https main-frame page of a screen: in the screen on the app's origin
    /// and on the sign-in paths of the Takosumi origins, else the browser view.
    static func mainFrameDecision(
        _ origin: String?, _ path: String?, _ origins: AppScreenOrigins
    ) -> AppScreenNavigation {
        guard let origin = origin else { return .browserView }
        if origins.accountsOrigins.contains(origin) {
            return accountsSignInPaths.contains(path ?? "") ? .inApp : .browserView
        }
        return origin == origins.appOrigin ? .inApp : .browserView
    }

    static func isTakosumiPageOutsideSignIn(
        _ origin: String?, _ path: String?, _ origins: AppScreenOrigins
    ) -> Bool {
        guard let origin = origin else { return false }
        return origins.accountsOrigins.contains(origin) && !accountsSignInPaths.contains(path ?? "")
    }

    /// The path as written (percent-encoded, dot segments kept).
    static func encodedPath(_ rawUrl: String) -> String? {
        URLComponents(string: rawUrl)?.percentEncodedPath
    }

    /// `rawUrl` as it may be shared or handed to the browser: credential-named
    /// query parameters with a value, undecodable or empty pairs and a fragment
    /// holding parameters are removed; the rest is kept as written.
    static func shareable(_ rawUrl: String) -> String {
        let hash = rawUrl.firstIndex(of: "#")
        let beforeHash = hash.map { String(rawUrl[..<$0]) } ?? rawUrl
        let fragment = hash.map { String(rawUrl[rawUrl.index(after: $0)...]) }
        let question = beforeHash.firstIndex(of: "?")
        let base = question.map { String(beforeHash[..<$0]) } ?? beforeHash
        let query = question.map { String(beforeHash[beforeHash.index(after: $0)...]) } ?? ""
        let kept = query.split(separator: "&", omittingEmptySubsequences: true).filter { pair in
            let parts = pair.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard let name = decodeQueryPart(String(parts[0])),
                  let value = decodeQueryPart(parts.count > 1 ? String(parts[1]) : "") else {
                return false
            }
            return !isCredential(name: name, value: value)
        }
        var shared = base
        if !kept.isEmpty { shared += "?" + kept.joined(separator: "&") }
        if let fragment = fragment, !fragment.contains("=") { shared += "#" + fragment }
        return shared
    }

    private static func isCredential(name: String, value: String) -> Bool {
        let normalized = String(name.lowercased().filter { $0.isASCII && ($0.isLowercase || $0.isNumber) })
        return !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty &&
            credentialQueryNames.contains(normalized)
    }

    /// https with a host and no user info: the only URLs a screen opens with.
    static func isLaunchable(_ rawUrl: String) -> Bool {
        guard let url = URL(string: rawUrl.trimmingCharacters(in: .whitespacesAndNewlines)),
              url.scheme?.lowercased() == "https" else {
            return false
        }
        return decide(rawUrl, mainFrame: true) == .inApp
    }

    /// True when a screen with `origins` shows `rawUrl` itself.
    static func isInScreen(_ rawUrl: String, origins: AppScreenOrigins?) -> Bool {
        isLaunchable(rawUrl) && decide(rawUrl, mainFrame: true, origins: origins) == .inApp
    }

    /// `scheme://host[:port]` as `URL.origin` writes it: lowercase, default port
    /// dropped. Nil without an authority or with user info. Read from the text:
    /// WebKit hands canonical URLs.
    static func origin(of rawUrl: String) -> String? {
        let text = rawUrl.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let separator = text.range(of: "://") else { return nil }
        let scheme = String(text[text.startIndex..<separator.lowerBound])
        guard let first = scheme.first, first.isASCII, first.isLetter,
              scheme.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || "+.-".contains($0)) }) else {
            return nil
        }
        let rest = text[separator.upperBound...]
        let authority = String(rest.prefix { !"/?#\\".contains($0) })
        guard !authority.isEmpty, !authority.contains("@") else { return nil }
        var host: String
        var port: String?
        if authority.hasPrefix("[") {
            guard let end = authority.firstIndex(of: "]") else { return nil }
            host = String(authority[...end])
            let tail = authority[authority.index(after: end)...]
            if !tail.isEmpty && !tail.hasPrefix(":") { return nil }
            port = tail.isEmpty ? nil : String(tail.dropFirst())
        } else if let colon = authority.lastIndex(of: ":") {
            host = String(authority[..<colon])
            port = String(authority[authority.index(after: colon)...])
        } else {
            host = authority
        }
        if port?.isEmpty == true { port = nil }
        guard !host.isEmpty else { return nil }
        let lowerScheme = scheme.lowercased()
        var shownPort = ""
        if let port = port {
            guard port.allSatisfy({ $0.isASCII && $0.isNumber }), let number = Int(port), number <= 65_535 else {
                return nil
            }
            let defaultPort = lowerScheme == "https" ? 443 : lowerScheme == "http" ? 80 : -1
            if number != defaultPort { shownPort = ":\(number)" }
        }
        host = host.lowercased()
        return "\(lowerScheme)://\(host)\(shownPort)"
    }

    /// Exactly an https origin (`https://host[:port]`), not a shell host.
    static func isHttpsOrigin(_ raw: String) -> Bool {
        guard let origin = origin(of: raw), origin == raw, raw.hasPrefix("https://"),
              let host = URL(string: raw)?.host else {
            return false
        }
        return !isShellHost(host)
    }

    /// A page the screen may reopen at: https on the app's own origin, no user
    /// info, no fragment, no credential-named query parameter with a value.
    static func isRememberable(_ rawUrl: String, appOrigin: String) -> Bool {
        if rawUrl.count > resumeURLMaxLength || rawUrl.contains("#") { return false }
        if !rawUrl.lowercased().hasPrefix("https://") { return false }
        if rawUrl.contains(where: { $0.isWhitespace }) { return false }
        guard let origin = origin(of: rawUrl), origin == appOrigin,
              let host = URL(string: rawUrl)?.host, !isShellHost(host) else {
            return false
        }
        guard let question = rawUrl.firstIndex(of: "?") else { return true }
        let query = rawUrl[rawUrl.index(after: question)...]
        for pair in query.split(separator: "&", omittingEmptySubsequences: true) {
            let parts = pair.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard let name = decodeQueryPart(String(parts[0])),
                  let value = decodeQueryPart(parts.count > 1 ? String(parts[1]) : "") else {
                return false
            }
            if isCredential(name: name, value: value) { return false }
        }
        return true
    }

    /// As URLSearchParams: '+' is a space, then percent-decoding.
    private static func decodeQueryPart(_ part: String) -> String? {
        part.replacingOccurrences(of: "+", with: " ").removingPercentEncoding
    }

    /// A publisher-domain https link (universal link).
    static func isHttpsLink(_ raw: String) -> Bool {
        guard raw.count <= clientLinkMaxLength, raw.hasPrefix("https://"),
              !raw.contains(where: { $0.isWhitespace || $0.unicodeScalars.contains { CharacterSet.controlCharacters.contains($0) } }),
              origin(of: raw) != nil,
              let host = URL(string: raw)?.host else {
            return false
        }
        return !isShellHost(host)
    }

    /// A custom-scheme link whose scheme Takosumi does not refuse.
    static func isCustomSchemeLink(_ raw: String) -> Bool {
        guard raw.count <= clientLinkMaxLength,
              !raw.contains(where: { $0.isWhitespace || $0.unicodeScalars.contains { CharacterSet.controlCharacters.contains($0) } }),
              let colon = raw.firstIndex(of: ":") else {
            return false
        }
        let scheme = String(raw[..<colon])
        guard (2...32).contains(scheme.count),
              let first = scheme.first, first.isASCII, first.isLowercase,
              scheme.allSatisfy({ $0.isASCII && ($0.isLowercase || $0.isNumber || "+.-".contains($0)) }) else {
            return false
        }
        return !blockedClientSchemes.contains(scheme)
    }

    /// A declared app's name: short, no control, bidi or zero-width characters.
    static func isClientName(_ raw: String) -> Bool {
        let name = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name.count <= 64 else { return false }
        return !name.unicodeScalars.contains { scalar in
            CharacterSet.controlCharacters.contains(scalar) ||
                (0x200B...0x200F).contains(scalar.value) ||
                (0x202A...0x202E).contains(scalar.value) ||
                (0x2066...0x2069).contains(scalar.value) ||
                scalar.value == 0xFEFF
        }
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
