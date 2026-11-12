import XCTest

final class WebHomeUITests: XCTestCase {
    // A preinstalled, attested whole Tauri App, not the tiny driver target.
    private let app = XCUIApplication(bundleIdentifier: "com.takosumi.fixture.simulator")
    private var origin = ""
    private var editor = ""

    override func setUpWithError() throws {
        continueAfterFailure = false
        origin = try XCTUnwrap(ProcessInfo.processInfo.environment["SUMI_WEB_HOME_URL"])
        editor = try XCTUnwrap(ProcessInfo.processInfo.environment["SUMI_WEB_EDITOR_URL"])
        XCTAssertTrue(origin.hasPrefix("https://127.0.0.1:"))
        app.launchArguments = ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]
        app.launch()
        XCTAssertTrue(app.buttons["保存して開く"].waitForExistence(timeout: 20), "The whole App did not render WebHome")
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

    private func expect(_ text: String) {
        XCTAssertTrue(app.webViews.staticTexts[text].firstMatch.waitForExistence(timeout: 15), text)
    }

    private func expectHost(_ address: String) throws {
        let url = try XCTUnwrap(URL(string: address))
        let host = try XCTUnwrap(url.host)
        let port = try XCTUnwrap(url.port)
        XCTAssertTrue(app.staticTexts["表示中のサイト \(host):\(port)"].waitForExistence(timeout: 10))
    }

    private func connection(_ name: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@ AND label CONTAINS %@", name, origin)).firstMatch
    }

    private func addConnection(_ name: String) {
        let nameInput = app.webViews.textFields.matching(NSPredicate(format: "placeholderValue == %@", "自分のTakosumi")).firstMatch
        let urlInput = app.webViews.textFields.matching(NSPredicate(format: "placeholderValue == %@", "https://app.takosumi.com")).firstMatch
        XCTAssertTrue(nameInput.waitForExistence(timeout: 10))
        nameInput.tap()
        nameInput.typeText(name)
        XCTAssertTrue(urlInput.exists)
        urlInput.tap()
        urlInput.typeText(origin)
        app.buttons["保存して開く"].tap()
        expect("Fixture signed out")
        expect("Remote has no App IPC")
    }

    private func link(_ name: String) {
        let link = app.webViews.links[name].firstMatch
        XCTAssertTrue(link.waitForExistence(timeout: 15), name)
        link.tap()
    }

    private func menu(_ name: String) {
        app.buttons["メニュー"].tap()
        let action = app.buttons[name].firstMatch
        XCTAssertTrue(action.waitForExistence(timeout: 10), name)
        action.tap()
    }

    private func picker() {
        menu("接続先を切り替える")
        XCTAssertTrue(app.buttons["保存して開く"].waitForExistence(timeout: 10))
    }

    private func reopen(_ name: String) {
        let button = connection(name)
        XCTAssertTrue(button.waitForExistence(timeout: 10), "Saved connection \(name) is missing")
        button.tap()
    }

    private var draft: XCUIElement { app.webViews.textFields["Draft"].firstMatch }

    private func expectDraft(_ value: String) {
        XCTAssertTrue(draft.waitForExistence(timeout: 15))
        let stored = XCTNSPredicateExpectation(predicate: NSPredicate { [self] _, _ in
            draft.value as? String == value
        }, object: nil)
        XCTAssertEqual(XCTWaiter.wait(for: [stored], timeout: 10), .completed, "Wrong profile's editor state")
        expect("Remote has no App IPC")
    }

    private func writeDraft(_ value: String) {
        expectDraft("")
        draft.tap()
        draft.typeText(value)
        expectDraft(value)
        // The real Web form keyboard may cover native menu actions; use the
        // ordinary page tap to resign it, never a script or coordinate tap.
        app.webViews.staticTexts["Notes editor"].firstMatch.tap()
    }

    private func liveDocument() -> String {
        let marker = app.webViews.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", "Document ")).firstMatch
        XCTAssertTrue(marker.waitForExistence(timeout: 10))
        let value = marker.label
        XCTAssertNotNil(UUID(uuidString: String(value.dropFirst("Document ".count))))
        return value
    }

    func testWholeAppConnectionsWebHomeAndResumeStayProfileBound() throws {
        addConnection("Work")
        try expectHost(origin)
        link("Sign in Work")
        expect("Fixture home Work")
        expect("Instance data instance-Work")
        link("Account settings")
        expect("Account settings Work")
        app.buttons["戻る"].tap()
        expect("Fixture home Work")
        link("📝 Notes")
        expect("Notes editor")
        try expectHost(editor)
        writeDraft("work-draft")
        link("Page two")
        expect("Notes page two")
        app.buttons["戻る"].tap()
        expect("Notes editor")
        expectDraft("work-draft")
        menu("アプリホームへ")
        expect("Fixture home Work")
        app.buttons["戻る"].tap()
        expectDraft("work-draft")
        let workDocument = liveDocument()

        picker()
        reopen("Work")
        expectDraft("work-draft")
        XCTAssertEqual(liveDocument(), workDocument, "Reopening the same connection replaced its live page")
        XCUIDevice.shared.press(.home)
        app.activate()
        expectDraft("work-draft")
        XCTAssertEqual(liveDocument(), workDocument, "Foreground resume replaced its live page")

        picker()
        addConnection("Personal")
        link("Sign in Personal")
        expect("Fixture home Personal")
        link("📝 Notes")
        expect("Notes editor")
        writeDraft("personal-draft")
        picker()
        reopen("Work")
        expectDraft("work-draft")
        menu("アプリホームへ")
        expect("Fixture home Work")
        app.buttons["戻る"].tap()
        expectDraft("work-draft")
        let beforeCold = liveDocument()
        picker()

        // This is actual process termination followed by a new launch. The
        // server stays alive; no connection/profile or page is pre-seeded.
        app.terminate()
        XCTAssertTrue(app.wait(for: .notRunning, timeout: 10), "The original App process did not terminate")
        app.launch()
        XCTAssertTrue(connection("Work").waitForExistence(timeout: 20))
        XCTAssertTrue(connection("Personal").exists)
        reopen("Work")
        expect("Notes editor")
        expectDraft("work-draft")
        try expectHost(editor)
        XCTAssertNotEqual(liveDocument(), beforeCold, "Cold launch reused the terminated document")
        menu("アプリホームへ")
        expect("Fixture home Work")
        picker()
        reopen("Personal")
        expectDraft("personal-draft")
        menu("アプリホームへ")
        expect("Fixture home Personal")
        app.buttons["閉じる"].tap()
        XCTAssertTrue(connection("Work").waitForExistence(timeout: 10))
        reopen("Work")
        app.buttons["閉じる"].tap()
        XCTAssertTrue(app.buttons["保存して開く"].waitForExistence(timeout: 10))

        // Both native screens are closed. The actual shared confirmation must
        // name Work before any deletion is authorized, preserving Personal.
        let clear = app.buttons["保存データを消去…"].firstMatch
        XCTAssertTrue(clear.waitForExistence(timeout: 15))
        clear.tap()
        expect("Workの保存データを消去しますか？")
        app.buttons["保存データを消去"].tap()
        expect("Workのサイトデータを消去しました。")
        reopen("Work")
        expect("Fixture signed out")
        try expectHost(origin)
        link("Sign in Work")
        expect("Fixture home Work")
        link("📝 Notes")
        expectDraft("")
        app.buttons["閉じる"].tap()
        XCTAssertTrue(connection("Personal").waitForExistence(timeout: 10))
        reopen("Personal")
        expect("Fixture home Personal")
        expect("Instance data instance-Personal")
        link("📝 Notes")
        expectDraft("personal-draft")
        app.buttons["閉じる"].tap()
        XCTAssertTrue(app.buttons["保存して開く"].waitForExistence(timeout: 10))
    }
}
