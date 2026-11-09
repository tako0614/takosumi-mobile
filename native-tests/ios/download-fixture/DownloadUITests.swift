import XCTest

final class DownloadUITests: XCTestCase {
    private let app = XCUIApplication(bundleIdentifier: "com.takosumi.fixture.download")
    private var lastClosedAt: Date?

    override func setUpWithError() throws {
        continueAfterFailure = false
        let url = try XCTUnwrap(ProcessInfo.processInfo.environment["SUMI_DOWNLOAD_URL"])
        app.launchEnvironment["SUMI_DOWNLOAD_URL"] = url
        app.launchArguments = ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        XCTAssertTrue(app.buttons["Work"].waitForExistence(timeout: 15))
        let configuration = app.staticTexts["fixture-receipt"].label
        let attachment = XCTAttachment(string: configuration)
        attachment.name = "Actual fixture bundle and Documents configuration"
        attachment.lifetime = .keepAlways
        add(attachment)
        let data = try XCTUnwrap(configuration.data(using: .utf8))
        let value = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(value["fileSharing"] as? Bool, true, "The installed fixture must expose its own Documents")
        XCTAssertEqual(value["documentsInPlace"] as? Bool, true)
        XCTAssertEqual(value["displayName"] as? String, "Download Fixture")
        XCTAssertEqual(value["seedPresent"] as? Bool, true)
    }

    override func tearDownWithError() throws {
        let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        screenshot.lifetime = .keepAlways
        add(screenshot)
        let hierarchy = XCTAttachment(string: app.debugDescription)
        hierarchy.lifetime = .keepAlways
        add(hierarchy)
        app.terminate()
    }

    private func tapLink(_ name: String) {
        if name != "Fixture sign in", let closed = lastClosedAt {
            // A normal later retry, after the product's three-second prompt
            // cooldown. Do not repeatedly tap a blocked link or reset policy.
            let ready = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                Date().timeIntervalSince(closed) >= 3.1
            }, object: nil)
            XCTAssertEqual(XCTWaiter.wait(for: [ready], timeout: 5), .completed)
        }
        let link = app.webViews.links[name].firstMatch
        XCTAssertTrue(link.waitForExistence(timeout: 15), name)
        link.tap()
    }

    private func acceptSave() {
        let alert = app.alerts["ファイルを保存"]
        XCTAssertTrue(alert.waitForExistence(timeout: 10))
        alert.buttons["保存"].tap()
    }

    private func closeScreen() {
        let close = app.buttons["閉じる"].firstMatch
        XCTAssertTrue(close.waitForExistence(timeout: 10))
        close.tap()
        XCTAssertTrue(app.buttons["Inspect files"].waitForExistence(timeout: 10))
        lastClosedAt = Date()
    }

    private func inspect(cookie: Bool, blob: Bool, closed: Bool = false) throws {
        app.buttons["Inspect files"].tap()
        let ready = XCTNSPredicateExpectation(predicate: NSPredicate { [self] _, _ in
            guard let data = app.staticTexts["fixture-receipt"].label.data(using: .utf8),
                  let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return false }
            return value["cookie"] as? Bool == cookie && value["blob"] as? Bool == blob &&
                value["temporaryCount"] as? Int == 0 && value["sentinel"] as? Bool == true &&
                value["closedDuringTransfer"] as? Bool == closed && value["closeTimedOut"] as? Bool == false &&
                value["closedDuringExport"] as? Bool == true && value["exportFileRetained"] as? Bool == true &&
                value["exportCloseTimedOut"] as? Bool == false
        }, object: nil)
        XCTAssertEqual(XCTWaiter.wait(for: [ready], timeout: 10), .completed, app.staticTexts["fixture-receipt"].label)
    }

    private func exportToFixtureDocuments(afterSessionClose: Bool = false) {
        // The iOS 26 share sheet exposes this action as a Cell, as captured in
        // the original failed XCTest hierarchy. Retain the older Button form;
        // do not select the nested title text or substitute an export result.
        let actionTypes = [XCUIElement.ElementType.cell.rawValue, XCUIElement.ElementType.button.rawValue] as NSArray
        let saveToFiles = app.descendants(matching: .any).matching(
            NSPredicate(format: "label == %@ AND elementType IN %@", "Save to Files", actionTypes)
        ).firstMatch
        // Standard English iOS fixture. Missing actions fail; no fake export.
        if !saveToFiles.waitForExistence(timeout: 5) { app.swipeUp() }
        XCTAssertTrue(saveToFiles.waitForExistence(timeout: 5))
        if afterSessionClose {
            // Keep the real activity pending while the host removes the
            // session and observes the input file. Final receipt also requires
            // that removal and byte observation actually happened.
            let started = Date()
            let retained = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                Date().timeIntervalSince(started) >= 1
            }, object: nil)
            XCTAssertEqual(XCTWaiter.wait(for: [retained], timeout: 3), .completed)
            XCTAssertTrue(saveToFiles.exists, "Session removal dismissed the OS export")
        }
        saveToFiles.tap()
        XCTContext.runActivity(named: "Immediately after the real Save to Files action") { _ in
            let screenshot = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
            screenshot.lifetime = .keepAlways
            add(screenshot)
        }
        let save = app.buttons["Save"].firstMatch
        XCTAssertTrue(save.waitForExistence(timeout: 10))
        // Files remembers the previously selected folder for the next export.
        if app.navigationBars["Download Fixture"].exists {
            save.tap()
            XCTAssertTrue(app.buttons["閉じる"].firstMatch.waitForExistence(timeout: 10))
            return
        }
        let local = app.otherElements["On My iPhone"].firstMatch
        let localButton = app.buttons["On My iPhone"].firstMatch
        let localText = app.staticTexts["On My iPhone"].firstMatch
        if !local.exists && !localButton.exists && !localText.exists {
            let browse = app.buttons["Browse"].firstMatch
            if browse.waitForExistence(timeout: 3) { browse.tap() }
        }
        if localButton.exists { localButton.tap() }
        else if localText.exists { localText.tap() }
        else { XCTAssertTrue(local.waitForExistence(timeout: 5)); local.tap() }
        let folder = app.staticTexts["Download Fixture"].firstMatch
        XCTAssertTrue(folder.waitForExistence(timeout: 5))
        folder.tap()
        XCTAssertTrue(save.waitForExistence(timeout: 5))
        save.tap()
        XCTAssertTrue(app.buttons["閉じる"].firstMatch.waitForExistence(timeout: 10))
    }

    func testOriginalResponsesExportAndRetireWithinTheirProfile() throws {
        try XCTContext.runActivity(named: "Cookie response: retain original file across session removal until OS export") { _ in
            app.buttons["Work, close during export"].tap()
            tapLink("Fixture sign in")
            XCTAssertTrue(app.webViews.staticTexts["Fixture signed in"].waitForExistence(timeout: 10))
            tapLink("Cookie attachment")
            acceptSave()
            exportToFixtureDocuments(afterSessionClose: true)
            closeScreen()
            try inspect(cookie: true, blob: false)
        }
        try XCTContext.runActivity(named: "Blob response: real page Blob and OS export bytes") { _ in
            app.buttons["Work"].tap()
            tapLink("Blob export")
            acceptSave()
            exportToFixtureDocuments()
            closeScreen()
            try inspect(cookie: true, blob: true)
        }
        try XCTContext.runActivity(named: "Other UUID profile has no inherited cookie") { _ in
            app.buttons["Personal"].tap()
            tapLink("Cookie attachment")
            XCTAssertTrue(app.webViews.staticTexts["Cookie denied"].waitForExistence(timeout: 10))
            XCTAssertFalse(app.alerts["ファイルを保存"].exists)
            closeScreen()
            try inspect(cookie: true, blob: true)
        }
        try XCTContext.runActivity(named: "Decline does not write a temporary file") { _ in
            app.buttons["Work"].tap()
            tapLink("Cookie attachment")
            let alert = app.alerts["ファイルを保存"]
            XCTAssertTrue(alert.waitForExistence(timeout: 10))
            alert.buttons["キャンセル"].tap()
            closeScreen()
            try inspect(cookie: true, blob: true)
        }
        try XCTContext.runActivity(named: "Cancel a slow actual response and retire its partial file") { _ in
            app.buttons["Work"].tap()
            tapLink("Slow attachment")
            acceptSave()
            let progress = app.alerts["ダウンロード中"]
            XCTAssertTrue(progress.waitForExistence(timeout: 10))
            let bytes = progress.staticTexts.matching(NSPredicate(format: "label MATCHES %@", "(?s)slow\\.bin\\n\\n[1-9][0-9,.]*\\s.*")).firstMatch
            XCTAssertTrue(bytes.waitForExistence(timeout: 10), "Real transfer progress must receive bytes before cancel")
            progress.buttons["キャンセル"].tap()
            closeScreen()
            try inspect(cookie: true, blob: true)
        }
        try XCTContext.runActivity(named: "Real session removal stops an active writer before cleanup") { _ in
            app.buttons["Work, close during transfer"].tap()
            tapLink("Slow attachment")
            acceptSave()
            XCTAssertTrue(app.buttons["Inspect files"].waitForExistence(timeout: 30))
            try inspect(cookie: true, blob: true, closed: true)
        }
    }
}
