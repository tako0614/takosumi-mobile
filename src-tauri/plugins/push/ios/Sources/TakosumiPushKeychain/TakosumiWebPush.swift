import CryptoKit
import Foundation

/// What a Takosumi notification shows and opens, read from its decrypted
/// Declarative Web Push message.
public struct TakosumiNotification {
    public let title: String
    public let body: String?
    /// The page to open, only when it is on one of the subscription's origins.
    public let navigate: String?
    public let tag: String?
}

public enum TakosumiWebPushError: Error {
    case malformed
    case unauthenticated
}

/// The receiving side of RFC 8291 (`aes128gcm`, one record) and the
/// Declarative Web Push message, the same rules as the Android plugin
/// (`WebPushCrypto.kt`, `DeclarativePush.kt`). Shared by the app (taps) and
/// the Notification Service Extension (step 8). A message that decrypts with
/// a subscription's key and `auth` is the only proof of which instance sent
/// it: the APNs envelope around it is set by the relay.
public enum TakosumiWebPush {
    public static let maxBodyBytes = 4096
    static let magic = 8030

    public static func decrypt(_ body: Data, subscription: TakosumiPushSubscription) throws -> Data {
        let bytes = [UInt8](body)
        guard bytes.count <= maxBodyBytes,
              bytes.count > 21 + 65 + 16,
              subscription.authSecret.count == 16,
              subscription.publicKey.count == 65
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
            let privateKey = try P256.KeyAgreement.PrivateKey(rawRepresentation: subscription.privateKey)
            let sender = try P256.KeyAgreement.PublicKey(x963Representation: senderKey)
            shared = try privateKey.sharedSecretFromKeyAgreement(with: sender).withUnsafeBytes { Data($0) }
        } catch {
            throw TakosumiWebPushError.malformed
        }
        // RFC 8291 section 3.3 / 3.4.
        let keyInfo = Data("WebPush: info".utf8) + Data([0]) + subscription.publicKey + senderKey
        let ikm = hmac(key: hmac(key: subscription.authSecret, data: shared), data: keyInfo + Data([1]))
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

    public static func parse(_ plaintext: Data, origins: [String]) -> TakosumiNotification? {
        guard plaintext.count <= maxBodyBytes,
              let root = (try? JSONSerialization.jsonObject(with: plaintext)) as? [String: Any],
              let number = root["web_push"] as? NSNumber,
              CFGetTypeID(number) == CFNumberGetTypeID(),
              !CFNumberIsFloatType(number),
              number.intValue == magic,
              let notification = root["notification"] as? [String: Any],
              let title = text(notification["title"], maxCharacters: 120)
        else { return nil }
        var navigate: String?
        if let candidate = notification["navigate"] as? String,
           candidate.count <= 2048,
           let origin = TakosumiPushOrigin.of(candidate),
           origins.contains(origin) {
            navigate = candidate
        }
        return TakosumiNotification(
            title: title,
            body: text(notification["body"], maxCharacters: 1000),
            navigate: navigate,
            tag: text(notification["tag"], maxCharacters: 64))
    }

    static func hmac(key: Data, data: Data) -> Data {
        Data(HMAC<SHA256>.authenticationCode(for: data, using: SymmetricKey(data: key)))
    }

    static func text(_ value: Any?, maxCharacters: Int) -> String? {
        guard let value = value as? String else { return nil }
        let single = value.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
        guard !single.isEmpty else { return nil }
        guard single.count > maxCharacters else { return single }
        return String(single.prefix(maxCharacters - 1)).trimmingCharacters(in: .whitespaces) + "…"
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
