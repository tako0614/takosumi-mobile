import Foundation
import XCTest
@testable import AppScreenPolicyCore

/// Runs `AppScreenPolicy` against `navigation-policy-cases.json` and
/// `resume-url-cases.json`, the tables `src/url-policy.ts` and
/// `AppScreenPolicy.kt` are tested with.
final class AppScreenPolicyFixtureTests: XCTestCase {
    func testDownloadsRequireVisibleWebHomeAndNativeFrameOrigin() {
        let origin = "https://app.example"
        let blob = "blob:https://app.example/1234"
        func allowed(_ url: String, source: String? = "https://app.example", page: String? = "https://app.example/editor",
                     explicit: Bool = true, webHome: Bool = true, visible: Bool = true) -> Bool {
            AppScreenPolicy.canDownload(url, sourceOrigin: source, page: page,
                                        explicitDownload: explicit, webHome: webHome, visible: visible)
        }
        XCTAssertTrue(allowed("https://files.example/export?format=pdf", explicit: false))
        XCTAssertTrue(allowed(blob))
        XCTAssertFalse(allowed(blob, explicit: false))
        XCTAssertFalse(allowed(blob, source: "https://other.example"))
        XCTAssertFalse(allowed(blob, source: origin + "/editor"))
        XCTAssertFalse(allowed(blob, source: nil))
        XCTAssertFalse(allowed(blob, page: nil))
        XCTAssertFalse(allowed(blob, visible: false))
        XCTAssertFalse(allowed(blob, webHome: false))
        for url in ["blob:null/id", "blob:https://ipc.localhost/id", "file:///private/data", "data:text/plain,body",
                    "http://app.example/file", "https://user:pass@app.example/file", "https://ipc.localhost/file"] {
            XCTAssertFalse(allowed(url), url)
        }
        XCTAssertFalse(allowed(blob, page: "https://ipc.localhost/"))
        XCTAssertEqual(AppScreenPolicy.decide(blob, mainFrame: true, origins: nil), .block)
        XCTAssertFalse(AppScreenPolicy.isLaunchable(blob))
    }

    func testSuggestedDownloadFilenameCannotChoosePathOrHideExtension() {
        XCTAssertEqual(AppScreenPolicy.downloadFilename("../../secrets/export.txt"), "export.txt")
        XCTAssertEqual(AppScreenPolicy.downloadFilename("C:\\private\\export.txt"), "export.txt")
        XCTAssertEqual(AppScreenPolicy.downloadFilename("\u{0}bad\nfile\u{202E}.txt"), "badfile.txt")
        XCTAssertEqual(AppScreenPolicy.downloadFilename(".."), "download")
        XCTAssertEqual(AppScreenPolicy.downloadFilename(" .hidden.txt "), "hidden.txt")
        XCTAssertEqual(AppScreenPolicy.downloadFilename("report:final.pdf"), "reportfinal.pdf")
    }

    func testAttachmentDownloadsEvenWhenWebKitCanRenderItsMimeType() {
        XCTAssertTrue(AppScreenPolicy.shouldDownloadResponse(canShowMIMEType: false, contentDisposition: nil))
        XCTAssertTrue(AppScreenPolicy.shouldDownloadResponse(canShowMIMEType: true, contentDisposition: "Attachment; filename=report.pdf"))
        XCTAssertFalse(AppScreenPolicy.shouldDownloadResponse(canShowMIMEType: true, contentDisposition: "inline; filename=attachment"))
        XCTAssertFalse(AppScreenPolicy.shouldDownloadResponse(canShowMIMEType: true, contentDisposition: nil))
    }

    func testLongUnicodeDownloadNameKeepsItsExtensionWithinByteLimit() {
        let name = AppScreenPolicy.downloadFilename(String(repeating: "資料🦀", count: 100) + ".pdf")
        XCTAssertLessThanOrEqual(name.utf8.count, 180)
        XCTAssertTrue(name.hasSuffix(".pdf"))
        XCTAssertFalse(name.contains("/"))
        XCTAssertFalse(name.isEmpty)
    }

    func testMediaPromptEligibilityIncludesOtherHttpsFrames() {
        // Cross-origin frames can ask through WebKit's own Permissions Policy;
        // no origin is automatically granted by this eligibility decision.
        for origin in ["https://app.example", "https://call.example:8443", "https://[::1]:8443"] {
            XCTAssertTrue(AppScreenPolicy.canPromptForMediaCapture(
                origin: origin, page: "https://app.example/room", visible: true), origin)
        }
    }

    func testMediaNativeOriginPreservesIpv6AndNonDefaultPorts() {
        XCTAssertEqual(AppScreenPolicy.mediaCaptureOrigin(scheme: "https", host: "call.example", port: 0), "https://call.example")
        XCTAssertEqual(AppScreenPolicy.mediaCaptureOrigin(scheme: "https", host: "call.example", port: 443), "https://call.example")
        XCTAssertEqual(AppScreenPolicy.mediaCaptureOrigin(scheme: "https", host: "call.example", port: 8443), "https://call.example:8443")
        XCTAssertEqual(AppScreenPolicy.mediaCaptureOrigin(scheme: "https", host: "::1", port: 8443), "https://[::1]:8443")
        XCTAssertEqual(AppScreenPolicy.mediaCaptureOrigin(scheme: "https", host: "[::1]", port: 8443), "https://[::1]:8443")
        for item in [("http", "call.example", 443), ("https", "", 443), ("https", "ipc.localhost", 443), ("https", "call.example", -1), ("https", "call.example", 65_536)] {
            XCTAssertNil(AppScreenPolicy.mediaCaptureOrigin(scheme: item.0, host: item.1, port: item.2))
        }
    }

    func testMediaPromptRefusesHiddenMissingAndNonWebDocuments() {
        let site = "https://call.example"
        XCTAssertFalse(AppScreenPolicy.canPromptForMediaCapture(origin: site, page: site, visible: false))
        XCTAssertFalse(AppScreenPolicy.canPromptForMediaCapture(origin: site, page: nil, visible: true))
        XCTAssertFalse(AppScreenPolicy.canPromptForMediaCapture(origin: nil, page: site, visible: true))
        for invalid in ["http://call.example", "about:blank", "file:///private/file", "https://ipc.localhost", "https://tauri.localhost", "https://user:pass@call.example"] {
            XCTAssertFalse(AppScreenPolicy.canPromptForMediaCapture(origin: invalid, page: site, visible: true), invalid)
            XCTAssertFalse(AppScreenPolicy.canPromptForMediaCapture(origin: site, page: invalid, visible: true), invalid)
        }
        for notAnOrigin in [site + "/room", site + "?token=secret", site + "#fragment"] {
            XCTAssertFalse(AppScreenPolicy.canPromptForMediaCapture(origin: notAnOrigin, page: site, visible: true), notAnOrigin)
        }
    }

    func testClearRemovesEveryScopeForTheExactKeyWithoutChangingOtherProfiles() {
        let key = "web-home:11111111-1111-4111-8111-111111111111:https://sumi.example"
        let personal = "web-home:22222222-2222-4222-8222-222222222222:https://sumi.example"
        let stored: [String: [String: Any]] = [
            "\(key)\n\(key)": ["url": "https://app.example/work"],
            "alternate-scope\n\(key)": ["url": "https://app.example/alternate"],
            "\(personal)\n\(personal)": ["url": "https://app.example/personal"],
            "legacy-scope\nlegacy-app": ["url": "https://app.example/legacy"],
            "unknown": ["url": "https://app.example/unknown"],
        ]
        let kept = AppScreenPolicy.clearingResumeMetadata(stored, key: key)
        XCTAssertEqual(kept.count, 3)
        XCTAssertEqual(kept["\(personal)\n\(personal)"]?["url"] as? String, "https://app.example/personal")
        XCTAssertEqual(kept["legacy-scope\nlegacy-app"]?["url"] as? String, "https://app.example/legacy")
        XCTAssertNotNil(kept["unknown"])
    }

    func testClearTargetsNeverSelectSharedOrAnotherConnection() throws {
        let cases = try XCTUnwrap(fixture("clear-profile-cases.json")["cases"] as? [[String: Any]])
        for item in cases {
            let profile = try XCTUnwrap(item["profile"] as? String)
            let key = try XCTUnwrap(item["key"] as? String)
            XCTAssertEqual(AppScreenPolicy.canClearProfile(profile, key: key), try XCTUnwrap(item["valid"] as? Bool), key)
        }
    }

    func testNativeNavigationOutcomesMatchSharedTransitions() throws {
        let events = try XCTUnwrap(fixture("resume-navigation-cases.json")["events"] as? [[String: Any]])
        var state = AppScreenResumeState()
        var observations = 0
        for item in events {
            let event = try XCTUnwrap(item["event"] as? String)
            let id = item["id"] as? Int ?? 0
            switch event {
            case "reset": state = AppScreenResumeState()
            case "begin": state.begin(id)
            case "commit": state.commit(id)
            case "finish": state.finish(id, success: item["success"] as? Bool ?? false, url: item["url"] as? String)
            case "observe":
                observations += 1
                let url = try XCTUnwrap(item["url"] as? String)
                XCTAssertEqual(state.canRemember(url), try XCTUnwrap(item["remember"] as? Bool), url)
            default: XCTFail("Unknown shared navigation event: \(event)")
            }
        }
        XCTAssertEqual(observations, 15)
    }

    func testCanonicalProfileNames() throws {
        let cases = try XCTUnwrap(fixture("profile-cases.json")["cases"] as? [[String: Any]])
        for item in cases {
            let profile = try XCTUnwrap(item["profile"] as? String)
            let valid = try XCTUnwrap(item["valid"] as? Bool)
            XCTAssertEqual(AppScreenPolicy.isWebProfile(profile), valid, profile)
        }
    }

    func testWebHomeKeepsWebNavigationButNotShellAccess() {
        XCTAssertTrue(AppScreenPolicy.isWebHomeURL("https://sumi.example/"))
        XCTAssertTrue(AppScreenPolicy.isInScreen("https://sumi.example/apps", origins: nil))
        XCTAssertTrue(AppScreenPolicy.isInScreen("https://app.example/inbox", origins: nil))
        XCTAssertFalse(AppScreenPolicy.isInScreen("https://ipc.localhost/", origins: nil))
        for url in ["https://sumi.example/?token=secret", "https://sumi.example/#secret", "https://sumi.example/apps", "https://ipc.localhost/"] {
            XCTAssertFalse(AppScreenPolicy.isWebHomeURL(url), url)
        }
    }

    private func fixture(_ name: String) throws -> [String: Any] {
        // Tests/AppScreenPolicyTests/<file> → the plugin root.
        var directory = URL(fileURLWithPath: #filePath)
        for _ in 0..<5 { directory.deleteLastPathComponent() }
        let data = try Data(contentsOf: directory.appendingPathComponent(name))
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    private func expected(_ decision: String) -> String { decision }

    private func name(_ navigation: AppScreenNavigation) -> String {
        switch navigation {
        case .inApp: return "in-app"
        case .browserView: return "browser-view"
        case .externalConfirm: return "external-confirm"
        case .block: return "block"
        }
    }

    func testEverySharedNavigationCase() throws {
        let cases = try XCTUnwrap(fixture("navigation-policy-cases.json")["cases"] as? [[String: Any]])
        XCTAssertGreaterThan(cases.count, 80)
        var mismatches: [String] = []
        for item in cases {
            let url = try XCTUnwrap(item["url"] as? String)
            let mainFrame = try XCTUnwrap(item["mainFrame"] as? Bool)
            let origins = (item["appOrigin"] as? String).map {
                AppScreenOrigins(
                    appOrigin: $0,
                    accountsOrigins: Set(item["accountsOrigins"] as? [String] ?? []))
            }
            let got = name(AppScreenPolicy.decide(url, mainFrame: mainFrame, origins: origins))
            let want = try XCTUnwrap(item["decision"] as? String)
            if got != want {
                mismatches.append("\(url) mainFrame=\(mainFrame) app=\(origins?.appOrigin ?? "-"): \(got) != \(want)")
            }
        }
        XCTAssertEqual(mismatches, [])
    }

    func testSignInPathsMatchTheTable() throws {
        let paths = try XCTUnwrap(fixture("navigation-policy-cases.json")["accountsSignInPaths"] as? [String])
        XCTAssertEqual(Set(paths), AppScreenPolicy.accountsSignInPaths)
    }

    func testEverySharedResumeAndShareCase() throws {
        let rules = try fixture("resume-url-cases.json")
        let names = try XCTUnwrap(rules["credentialQueryNames"] as? [String])
        XCTAssertEqual(Set(names), AppScreenPolicy.credentialQueryNames)
        for item in try XCTUnwrap(rules["cases"] as? [[String: Any]]) {
            let url = try XCTUnwrap(item["url"] as? String)
            let origin = try XCTUnwrap(item["appOrigin"] as? String)
            let remember = try XCTUnwrap(item["remember"] as? Bool)
            XCTAssertEqual(AppScreenPolicy.isRememberable(url, appOrigin: origin), remember, url)
        }
        for item in try XCTUnwrap(rules["shareCases"] as? [[String: Any]]) {
            let url = try XCTUnwrap(item["url"] as? String)
            XCTAssertEqual(AppScreenPolicy.shareable(url), item["shared"] as? String, url)
        }
    }

    func testWebHomeResumeCases() throws {
        let rules = try fixture("web-home-resume-cases.json")
        for item in try XCTUnwrap(rules["cases"] as? [[String: Any]]) {
            let url = try XCTUnwrap(item["url"] as? String)
            let remember = try XCTUnwrap(item["remember"] as? Bool)
            XCTAssertEqual(AppScreenPolicy.isRememberable(url, appOrigin: "https://sumi.example", webHome: true), remember, url)
        }
        XCTAssertFalse(AppScreenPolicy.isRememberable("https://office.example/document/1", appOrigin: "https://sumi.example"))
    }

    func testOriginsAndLinks() {
        XCTAssertEqual(AppScreenPolicy.origin(of: "https://APP.example:443/x?y"), "https://app.example")
        XCTAssertEqual(AppScreenPolicy.origin(of: "https://[::1]:8443/x"), "https://[::1]:8443")
        XCTAssertNil(AppScreenPolicy.origin(of: "https://u@app.example/"))
        XCTAssertTrue(AppScreenPolicy.isHttpsOrigin("https://accounts.example"))
        XCTAssertFalse(AppScreenPolicy.isHttpsOrigin("https://accounts.example/"))
        XCTAssertFalse(AppScreenPolicy.isHttpsOrigin("https://ipc.localhost"))
        XCTAssertTrue(AppScreenPolicy.isShellHost("custom-protocol.localhost"))
        XCTAssertFalse(AppScreenPolicy.isShellHost("localhost"))
        XCTAssertTrue(AppScreenPolicy.isCustomSchemeLink("yurucommu://connect?host_url=x"))
        for bad in ["takosumi://x", "https://x.example/", "tel:+81", "Yurucommu://x", "y://x", "googlechrome://x"] {
            XCTAssertFalse(AppScreenPolicy.isCustomSchemeLink(bad), bad)
        }
        XCTAssertTrue(AppScreenPolicy.isHttpsLink("https://open.example/connect#host_url=x"))
        XCTAssertFalse(AppScreenPolicy.isHttpsLink("https://tauri.localhost/"))
        XCTAssertTrue(AppScreenPolicy.isClientName("Yurucommu"))
        XCTAssertFalse(AppScreenPolicy.isClientName("Chat\u{202E}exe"))
    }
}
