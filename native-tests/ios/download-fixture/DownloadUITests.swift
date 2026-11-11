import XCTest

final class DownloadUITests: XCTestCase {
    private let app = XCUIApplication(bundleIdentifier: "com.takosumi.fixture.download")
    private let filesApp = XCUIApplication(bundleIdentifier: "com.apple.DocumentsApp")
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
        // The wait's own verdict never changes; on a painted page that still
        // misses its link, the counts name which mechanism actually failed:
        // an extra full-screen window, a second live webview, or a webview
        // whose web-content accessibility never arrived (links stays 0 while
        // the native bar and scroll bars still enumerate).
        // The screen's own Progress bar separates the two observed variants:
        // "still loading" (run 42797894615: 10%, zero server requests) from
        // "loaded but a11y-empty" (run 42729892985: page painted and served).
        XCTAssertTrue(link.waitForExistence(timeout: 15),
                      "\(name); windows=\(app.windows.count) webViews=\(app.webViews.count) links=\(app.webViews.links.count) staticTexts=\(app.webViews.staticTexts.count) progress=\((app.progressIndicators["Progress"].firstMatch.value as? String) ?? "absent")")
        link.tap()
    }

    private func acceptSave() {
        let alert = app.alerts["ファイルを保存"]
        XCTAssertTrue(alert.waitForExistence(timeout: 10))
        alert.buttons["保存"].tap()
    }

    private func closeScreen() {
        let close = app.buttons["閉じる"].firstMatch
        // An OS export surface can still be dismissing when the element
        // already exists: its tap then computes hit point {-1,-1} and lands
        // nowhere (run 42971293681: the Files save kept the sheet up for
        // several more seconds). Wait until the real control is hittable —
        // same bound, and still an actual tap on the actual button.
        let deadline = Date().addingTimeInterval(10)
        while !close.isHittable && Date() < deadline {
            Thread.sleep(forTimeInterval: 0.1)
        }
        XCTAssertTrue(close.isHittable, "閉じる stayed covered; the close tap could not land")
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
        // The picker can reopen one level above the remembered folder — a
        // location level where Save legitimately does not exist until a
        // folder is chosen — and its remote view takes seconds to attach
        // after a system stall (run 42961067390: the picker materialized
        // past the old 10 s Save wait while its remote hierarchy errored).
        // Wait for the picker itself before looking for any control in it.
        filesElement("OS Files export picker did not open", timeout: 30) {
            $0.otherElements["Browse View (Picker)"].firstMatch
        }
        // Files remembers the previously selected folder for the next export.
        if filesExists({ $0.navigationBars["Download Fixture"] }) {
            filesElement("OS Files export Save did not open") { $0.buttons["Save"].firstMatch }.tap()
            XCTAssertTrue(app.buttons["閉じる"].firstMatch.waitForExistence(timeout: 10))
            return
        }
        let localElement: (XCUIApplication) -> XCUIElement = { host in
            let button = host.buttons["On My iPhone"].firstMatch
            if button.exists { return button }
            let text = host.staticTexts["On My iPhone"].firstMatch
            if text.exists { return text }
            return host.otherElements["On My iPhone"].firstMatch
        }
        // Already inside On My iPhone: its nav-bar title is not a row and
        // must not be tapped as one; the folder list is reached directly.
        if !filesExists({ $0.navigationBars.staticTexts["On My iPhone"].firstMatch }) {
            // Browse is an optional route inside the same real picker; tap it
            // only when either hierarchy actually shows it.
            if let browse = filesOptional({ $0.buttons["Browse"].firstMatch }, timeout: 3) { browse.tap() }
            filesElement("Files On My iPhone did not appear", timeout: 5) { localElement($0) }.tap()
        }
        filesElement("Download Fixture folder did not appear", timeout: 5) { $0.staticTexts["Download Fixture"].firstMatch }.tap()
        filesElement("OS Files export Save did not remain", timeout: 5) { $0.buttons["Save"].firstMatch }.tap()
        XCTAssertTrue(app.buttons["閉じる"].firstMatch.waitForExistence(timeout: 10))
    }

    // The iOS 26 Files export picker is hosted by com.apple.DocumentsApp. Its
    // controls can surface inside the fixture's remote-view hierarchy or only
    // in the provider's own process, depending on presentation. Query both real
    // hierarchies within the same bound; a missing control stays a failure and
    // never substitutes an export result for a touch.
    private func filesElement(_ what: String, timeout: TimeInterval = 10, _ query: (XCUIApplication) -> XCUIElement) -> XCUIElement {
        var element = query(app)
        let deadline = Date().addingTimeInterval(timeout)
        while !element.exists && Date() < deadline {
            Thread.sleep(forTimeInterval: 0.1)
            let remote = query(filesApp)
            element = remote.exists ? remote : query(app)
        }
        XCTAssertTrue(element.exists, what)
        return element
    }

    private func filesOptional(_ query: (XCUIApplication) -> XCUIElement, timeout: TimeInterval) -> XCUIElement? {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let element = query(app)
            if element.exists { return element }
            let remote = query(filesApp)
            if remote.exists { return remote }
            Thread.sleep(forTimeInterval: 0.1)
        }
        let element = query(app)
        return element.exists ? element : (query(filesApp).exists ? query(filesApp) : nil)
    }

    private func filesExists(_ query: (XCUIApplication) -> XCUIElement) -> Bool {
        query(app).exists || query(filesApp).exists
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
