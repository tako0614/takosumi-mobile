#if canImport(CryptoKit)
import CryptoKit
#else
// Linux (the shared-fixture tests in ../../WebPushTests): swift-crypto has the
// same API.
import Crypto
#endif
import Foundation

/// One Takosumi instance subscription: the relay's subscription id (`s`), the
/// instance and account it belongs to, the origins its notifications may open,
/// and the RFC 8291 key material.
public struct TakosumiPushSubscription: Codable {
    public let subscriptionId: String
    public let instance: String
    public let account: String
    public let label: String
    public let origins: [String]
    /// Uncompressed P-256 point (`p256dh`), 65 bytes.
    public let publicKey: Data
    /// The 16-byte `auth` secret.
    public let authSecret: Data
    /// `P256.KeyAgreement.PrivateKey.rawRepresentation`.
    public let privateKey: Data

    public init(
        subscriptionId: String,
        instance: String,
        account: String,
        label: String,
        origins: [String],
        publicKey: Data,
        authSecret: Data,
        privateKey: Data
    ) {
        self.subscriptionId = subscriptionId
        self.instance = instance
        self.account = account
        self.label = label
        self.origins = origins
        self.publicKey = publicKey
        self.authSecret = authSecret
        self.privateKey = privateKey
    }
}

/// What a Takosumi notification shows and opens, read from its decrypted
/// Declarative Web Push message.
public struct TakosumiNotification {
    public let title: String
    public let body: String?
    /// The page to open, only when it is on one of the subscription's origins.
    public let navigate: String?
    public let lang: String?
    public let tag: String?
}

public enum TakosumiWebPushError: Error {
    case malformed
    case unauthenticated
}

/// The receiving side of RFC 8291 (`aes128gcm`, one record) and the
/// Declarative Web Push message, the same rules as the Android plugin
/// (`WebPushCrypto.kt`, `DeclarativePush.kt`) and checked against the same
/// table (`web-push-cases.json`, ../../WebPushTests). Shared by the app (taps)
/// and the Notification Service Extension (step 8). A message that decrypts
/// with a subscription's key and `auth` is the only proof of which instance
/// sent it: the APNs envelope around it is set by the relay.
public enum TakosumiWebPush {
    public static let maxBodyBytes = 4096
    static let magic = 8030

    public static func decrypt(_ body: Data, subscription: TakosumiPushSubscription) throws -> Data {
        try decrypt(
            body,
            privateKey: subscription.privateKey,
            publicKey: subscription.publicKey,
            authSecret: subscription.authSecret)
    }

    /// `privateKey` is the raw 32-byte scalar (`rawRepresentation`).
    public static func decrypt(_ body: Data, privateKey rawKey: Data, publicKey: Data, authSecret: Data) throws -> Data {
        let bytes = [UInt8](body)
        guard bytes.count <= maxBodyBytes,
              bytes.count > 21 + 65 + 16,
              authSecret.count == 16,
              publicKey.count == 65
        else { throw TakosumiWebPushError.malformed }
        let salt = Data(bytes[0..<16])
        let recordSize = UInt32(bytes[16]) << 24 | UInt32(bytes[17]) << 16 | UInt32(bytes[18]) << 8 | UInt32(bytes[19])
        guard bytes[20] == 65, recordSize >= 18 else { throw TakosumiWebPushError.malformed }
        let senderKey = Data(bytes[21..<86])
        let record = Data(bytes[86...])
        // One record only, as the relay enforces.
        guard UInt64(record.count) <= UInt64(recordSize) else { throw TakosumiWebPushError.malformed }

        let shared: Data
        do {
            let privateKey = try P256.KeyAgreement.PrivateKey(rawRepresentation: rawKey)
            let sender = try P256.KeyAgreement.PublicKey(x963Representation: senderKey)
            shared = try privateKey.sharedSecretFromKeyAgreement(with: sender).withUnsafeBytes { Data($0) }
        } catch {
            throw TakosumiWebPushError.malformed
        }
        // RFC 8291 section 3.3 / 3.4.
        let keyInfo = Data("WebPush: info".utf8) + Data([0]) + publicKey + senderKey
        let ikm = hmac(key: hmac(key: authSecret, data: shared), data: keyInfo + Data([1]))
        let prk = hmac(key: salt, data: ikm)
        let cek = hmac(key: prk, data: Data("Content-Encoding: aes128gcm".utf8) + Data([0, 1])).prefix(16)
        let nonce = hmac(key: prk, data: Data("Content-Encoding: nonce".utf8) + Data([0, 1])).prefix(12)

        let plain: Data
        do {
            let box = try AES.GCM.SealedBox(
                nonce: AES.GCM.Nonce(data: nonce),
                ciphertext: record.dropLast(16),
                tag: record.suffix(16))
            plain = try AES.GCM.open(box, using: SymmetricKey(data: cek))
        } catch {
            throw TakosumiWebPushError.unauthenticated
        }
        // Data, the last-record delimiter 0x02, then zeros.
        guard let last = plain.lastIndex(where: { $0 != 0 }), plain[last] == 2 else {
            throw TakosumiWebPushError.malformed
        }
        return Data(plain[plain.startIndex..<last])
    }

    /// The Declarative Web Push message, or nil when this is not one: the
    /// `web_push` member must be the integer 8030 and `title` a non-empty
    /// string. Other members of the wrong type are left out, as on Android.
    public static func parse(_ plaintext: Data, origins: [String]) -> TakosumiNotification? {
        guard plaintext.count <= maxBodyBytes,
              let message = try? JSONDecoder().decode(DeclarativeMessage.self, from: plaintext),
              message.magic == magic,
              let title = text(message.notification.title, maxScalars: 120)
        else { return nil }
        var navigate: String?
        if let candidate = message.notification.navigate,
           candidate.utf8.count <= 2048,
           let origin = TakosumiPushOrigin.of(candidate),
           origins.contains(origin) {
            navigate = candidate
        }
        let lang = message.notification.lang.flatMap { value in
            value.range(of: #"^[A-Za-z]{2,8}(-[A-Za-z0-9]{1,8}){0,7}$"#, options: .regularExpression) == nil ? nil : value
        }
        return TakosumiNotification(
            title: title,
            body: text(message.notification.body, maxScalars: 1000),
            navigate: navigate,
            lang: lang,
            tag: text(message.notification.tag, maxScalars: 64))
    }

    static func hmac(key: Data, data: Data) -> Data {
        Data(HMAC<SHA256>.authenticationCode(for: data, using: SymmetricKey(data: key)))
    }

    /// Whitespace runs (Java's `\s`: space, tab, LF, VT, FF, CR) become one
    /// space, ends trimmed; longer than `maxScalars` code points ends in "…".
    static func text(_ value: String?, maxScalars: Int) -> String? {
        guard let value = value else { return nil }
        let whitespace: Set<Unicode.Scalar> = [" ", "\t", "\n", "\u{0B}", "\u{0C}", "\r"]
        let words = value.unicodeScalars.split(whereSeparator: { whitespace.contains($0) })
        let single = words.map { String(String.UnicodeScalarView($0)) }.joined(separator: " ")
        guard !single.isEmpty else { return nil }
        let scalars = Array(single.unicodeScalars)
        guard scalars.count > maxScalars else { return single }
        var kept = String.UnicodeScalarView(scalars.prefix(maxScalars - 1))
        while let last = kept.last, whitespace.contains(last) { kept.removeLast() }
        return String(kept) + "…"
    }
}

/// `{"web_push": 8030, "notification": {...}}` with each notification member
/// read only when it is a string.
private struct DeclarativeMessage: Decodable {
    let magic: Int
    let notification: Fields

    enum CodingKeys: String, CodingKey {
        case magic = "web_push"
        case notification
    }

    struct Fields: Decodable {
        let title: String?
        let body: String?
        let navigate: String?
        let lang: String?
        let tag: String?

        enum CodingKeys: String, CodingKey {
            case title, body, navigate, lang, tag
        }

        init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            title = try? container.decode(String.self, forKey: .title)
            body = try? container.decode(String.self, forKey: .body)
            navigate = try? container.decode(String.self, forKey: .navigate)
            lang = try? container.decode(String.self, forKey: .lang)
            tag = try? container.decode(String.self, forKey: .tag)
        }
    }
}

public enum TakosumiPushOrigin {
    /// `scheme://host[:port]` of an https URL (or http on loopback), without
    /// user info; nil otherwise.
    public static func of(_ value: String) -> String? {
        guard let components = URLComponents(string: value),
              let scheme = components.scheme?.lowercased(),
              let host = components.host?.lowercased(), !host.isEmpty,
              components.user == nil, components.password == nil
        else { return nil }
        let loopback = ["localhost", "127.0.0.1", "::1"].contains(host)
        guard scheme == "https" || (scheme == "http" && loopback) else { return nil }
        let authority = host.contains(":") ? "[\(host)]" : host
        if let port = components.port, port != (scheme == "https" ? 443 : 80) {
            return "\(scheme)://\(authority):\(port)"
        }
        return "\(scheme)://\(authority)"
    }

    public static func base64UrlDecode(_ text: String) -> Data? {
        var base64 = text.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        while base64.count % 4 != 0 { base64 += "=" }
        return Data(base64Encoded: base64)
    }
}
