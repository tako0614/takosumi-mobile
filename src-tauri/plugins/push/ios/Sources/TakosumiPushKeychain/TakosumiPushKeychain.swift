import CryptoKit
import Foundation
import Security

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
}

public enum TakosumiPushKeychainError: Error {
    case exists
    case unavailable(OSStatus)
}

/// The subscriptions in the Keychain. Items are readable after the first
/// unlock (pushes arrive while the phone is locked) and never leave this
/// device. With `TakosumiPushKeychainAccessGroup` in Info.plist (for example
/// `$(AppIdentifierPrefix)com.takosumi.push`, also listed in the app's and the
/// extension's `keychain-access-groups` entitlement) the Notification Service
/// Extension reads the same items to decrypt before the notification shows.
public enum TakosumiPushKeychain {
    private static let service = "com.takosumi.push.subscriptions"

    public static var accessGroup: String? {
        guard let value = Bundle.main.object(
            forInfoDictionaryKey: "TakosumiPushKeychainAccessGroup") as? String,
            !value.isEmpty, !value.contains("$(")
        else { return nil }
        return value
    }

    private static func baseQuery() -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
        ]
        if let group = accessGroup { query[kSecAttrAccessGroup as String] = group }
        return query
    }

    public static func create(
        subscriptionId: String,
        instance: String,
        account: String,
        label: String,
        origins: [String]
    ) throws -> TakosumiPushSubscription {
        if find(subscriptionId) != nil { throw TakosumiPushKeychainError.exists }
        let key = P256.KeyAgreement.PrivateKey()
        var auth = Data(count: 16)
        let status = auth.withUnsafeMutableBytes { buffer in
            SecRandomCopyBytes(kSecRandomDefault, 16, buffer.baseAddress!)
        }
        guard status == errSecSuccess else { throw TakosumiPushKeychainError.unavailable(status) }
        let subscription = TakosumiPushSubscription(
            subscriptionId: subscriptionId,
            instance: instance,
            account: account,
            label: label,
            origins: origins,
            publicKey: key.publicKey.x963Representation,
            authSecret: auth,
            privateKey: key.rawRepresentation)
        var query = baseQuery()
        query[kSecAttrAccount as String] = subscriptionId
        query[kSecValueData as String] = try JSONEncoder().encode(subscription)
        query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let added = SecItemAdd(query as CFDictionary, nil)
        guard added == errSecSuccess else { throw TakosumiPushKeychainError.unavailable(added) }
        return subscription
    }

    public static func find(_ subscriptionId: String) -> TakosumiPushSubscription? {
        var query = baseQuery()
        query[kSecAttrAccount as String] = subscriptionId
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data
        else { return nil }
        return try? JSONDecoder().decode(TakosumiPushSubscription.self, from: data)
    }

    public static func list() -> [TakosumiPushSubscription] {
        var query = baseQuery()
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitAll
        var result: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let items = result as? [Data]
        else { return [] }
        return items.compactMap { try? JSONDecoder().decode(TakosumiPushSubscription.self, from: $0) }
    }

    public static func delete(_ subscriptionId: String) {
        var query = baseQuery()
        query[kSecAttrAccount as String] = subscriptionId
        SecItemDelete(query as CFDictionary)
    }
}
