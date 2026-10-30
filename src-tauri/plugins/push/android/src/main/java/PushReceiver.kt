package com.takosumi.push

import android.content.Context

/** What one relay message turned into. */
internal sealed class ReceivedMessage {
    /** A Takosumi notification from a known subscription. */
    class Shown(val record: SubscriptionRecord, val notification: DeclarativeNotification) : ReceivedMessage()

    /** A known subscription whose message could not be read. */
    class Unreadable(val record: SubscriptionRecord) : ReceivedMessage()

    /** No subscription with this `s` on this device. */
    object Unknown : ReceivedMessage()
}

/**
 * Turns the relay's data message (`s`: subscription id, `b`: the encrypted
 * Web Push body, base64url) into a notification. Decryption runs here, on the
 * device; the relay, Google and Apple only ever carry ciphertext.
 */
internal class PushReceiver(private val context: Context) {
    private val store = PushStore(context)
    private val notifier = PushNotifier(context)

    fun handle(data: Map<String, String>) {
        val now = System.currentTimeMillis()
        when (val message = read(data)) {
            is ReceivedMessage.Shown -> {
                val token = store.rememberTap(message.record, message.notification.navigate, now)
                notifier.show(message.record, message.notification, token)
                PushRuntime.publishReceived(message.record)
            }
            is ReceivedMessage.Unreadable -> {
                val token = store.rememberTap(message.record, null, now)
                notifier.showGeneric(message.record, token)
                PushRuntime.publishReceived(message.record)
            }
            ReceivedMessage.Unknown -> {
                notifier.showGeneric(null, store.rememberTap(null, null, now))
            }
        }
    }

    private fun read(data: Map<String, String>): ReceivedMessage {
        val subscriptionId = data["s"] ?: return ReceivedMessage.Unknown
        val record = store.find(subscriptionId) ?: return ReceivedMessage.Unknown
        return try {
            val body = PushStore.fromBase64Url(data["b"] ?: return ReceivedMessage.Unreadable(record))
            val plaintext = WebPushCrypto.decrypt(
                body = body,
                uaPrivate = WebPushCrypto.privateFromPkcs8(store.privateKey(record)),
                uaPublic = record.publicKey,
                authSecret = record.authSecret,
            )
            val notification = DeclarativePush.parse(plaintext, record.origins)
                ?: return ReceivedMessage.Unreadable(record)
            ReceivedMessage.Shown(record, notification)
        } catch (error: Exception) {
            // Bad base64, a key the Keystore lost, or a message that does not
            // decrypt: generic text, nothing from the message.
            ReceivedMessage.Unreadable(record)
        }
    }
}
