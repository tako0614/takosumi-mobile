import Foundation
import XCTest
@testable import AppScreenPolicyCore

/// Runs `AppScreenPolicy` against `navigation-policy-cases.json` and
/// `resume-url-cases.json`, the tables `src/url-policy.ts` and
/// `AppScreenPolicy.kt` are tested with.
final class AppScreenPolicyFixtureTests: XCTestCase {
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
