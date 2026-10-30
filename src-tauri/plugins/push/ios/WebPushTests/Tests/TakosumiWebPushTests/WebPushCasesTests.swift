import Foundation
import XCTest
import TakosumiWebPushCore

/// Runs `TakosumiWebPush` against `web-push-cases.json`: the RFC 8291
/// Appendix A message and the cases the Android plugin (`WebPushCasesTest`)
/// and `tests/push-cases.test.ts` check.
final class WebPushCasesTests: XCTestCase {
    private func table() throws -> [String: Any] {
        // Tests/TakosumiWebPushTests/<file> → the plugin root.
        var directory = URL(fileURLWithPath: #filePath)
        for _ in 0..<5 { directory.deleteLastPathComponent() }
        let data = try Data(contentsOf: directory.appendingPathComponent("web-push-cases.json"))
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    private func bytes(_ value: Any?) throws -> Data {
        try XCTUnwrap(TakosumiPushOrigin.base64UrlDecode(XCTUnwrap(value as? String)))
    }

    func testTheRfc8291AppendixAMessage() throws {
        let rfc = try XCTUnwrap(table()["rfc8291"] as? [String: Any])
        let plaintext = try TakosumiWebPush.decrypt(
            bytes(rfc["body"]),
            privateKey: bytes(rfc["uaPrivate"]),
            publicKey: bytes(rfc["uaPublic"]),
            authSecret: bytes(rfc["authSecret"]))
        XCTAssertEqual(String(decoding: plaintext, as: UTF8.self), rfc["plaintext"] as? String)
    }

    func testEverySharedCase() throws {
        let table = try table()
        let subscription = try XCTUnwrap(table["subscription"] as? [String: Any])
        let origins = try XCTUnwrap(subscription["origins"] as? [String])
        let cases = try XCTUnwrap(table["cases"] as? [[String: Any]])
        XCTAssertGreaterThanOrEqual(cases.count, 10)
        for item in cases {
            let name = try XCTUnwrap(item["name"] as? String)
            let auth = try bytes(item["authSecret"] ?? subscription["authSecret"])
            let plaintext: Data
            do {
                plaintext = try TakosumiWebPush.decrypt(
                    bytes(item["body"]),
                    privateKey: bytes(subscription["uaPrivate"]),
                    publicKey: bytes(subscription["uaPublic"]),
                    authSecret: auth)
            } catch {
                XCTAssertEqual(item["decrypts"] as? Bool, false, "\(name): \(error)")
                continue
            }
            XCTAssertEqual(item["decrypts"] as? Bool, true, "\(name): decrypted")
            XCTAssertEqual(String(decoding: plaintext, as: UTF8.self), item["plaintext"] as? String, name)
            let parsed = TakosumiWebPush.parse(plaintext, origins: origins)
            guard let expected = item["notification"] as? [String: Any] else {
                XCTAssertNil(parsed, "\(name): shown")
                continue
            }
            let notification = try XCTUnwrap(parsed, "\(name): not shown")
            XCTAssertEqual(notification.title, expected["title"] as? String, name)
            XCTAssertEqual(notification.body, expected["body"] as? String, name)
            XCTAssertEqual(notification.navigate, expected["navigate"] as? String, name)
            XCTAssertEqual(notification.lang, expected["lang"] as? String, name)
            XCTAssertEqual(notification.tag, expected["tag"] as? String, name)
        }
    }

    func testOriginsDropTheDefaultPortAndFoldCase() {
        XCTAssertEqual(TakosumiPushOrigin.of("https://TAKOSUMI.example:443/runs/1"), "https://takosumi.example")
        XCTAssertEqual(TakosumiPushOrigin.of("http://localhost:8787/x"), "http://localhost:8787")
        XCTAssertEqual(TakosumiPushOrigin.of("https://takosumi.example:8443/"), "https://takosumi.example:8443")
        XCTAssertNil(TakosumiPushOrigin.of("http://takosumi.example/"))
        XCTAssertNil(TakosumiPushOrigin.of("https://user@takosumi.example/"))
        XCTAssertNil(TakosumiPushOrigin.of("javascript:alert(1)"))
    }

    func testLongTextIsShortenedByCodePoints() throws {
        let long = String(repeating: "あ", count: 500)
        let message = #"{"web_push":8030,"notification":{"title":"\#(long)","body":"\#(long)","navigate":"https://t.example/"}}"#
        let parsed = try XCTUnwrap(TakosumiWebPush.parse(Data(message.utf8), origins: ["https://t.example"]))
        XCTAssertEqual(parsed.title.unicodeScalars.count, 120)
        XCTAssertTrue(parsed.title.hasSuffix("…"))
        XCTAssertEqual(parsed.body?.unicodeScalars.count, 500)
        XCTAssertNil(TakosumiWebPush.parse(Data([0x7B, 0xFF, 0x7D]), origins: ["https://t.example"]))
    }
}
