package com.takosumi.push

import android.content.Context

/** What one relay message turned into. */
internal sealed class ReceivedMessage {
    /** A Takosumi notification this device decrypted with a subscription's key. */
    class Shown(val record: SubscriptionRecord, val notification: DeclarativeNotification) : ReceivedMessage()

    /** A known subscription whose message could not be read. */
    class Unreadable(val record: SubscriptionRecord) : ReceivedMessage()

    /** No `s`, or no subscription with this `s` on this device: dropped. */
    object Unknown : ReceivedMessage()

    /** The subscriptions could not be read just now. */
    object StoreUnavailable : ReceivedMessage()
}

/**
 * Turns the relay's data message (`s`: subscription id, `b`: the encrypted
 * Web Push body, base64url) into a notification. Decryption runs here, on the
 * device; the relay, Google and Apple only ever carry ciphertext.
 *
 * The envelope (`s`) is not authenticated: only a message that decrypts with
 * the subscription's key and `auth` proves it came from that instance. So
 * only such a message records a tap that switches account and opens a page;
 * an unreadable one shows generic text whose tap just opens the app, and one
 * for an unknown subscription is dropped (it would only crowd out real taps).
 */
internal class PushReceiver(private val context: Context) {
    private val store = PushStore(context)
    private val notifier = PushNotifier(context)

    fun handle(data: Map<String, String>) {
        val now = System.currentTimeMillis()
        when (val message = read(data)) {
            is ReceivedMessage.Shown -> {
                // A storage failure must not take the notification (or the
                // service) down: it then opens the app without a page.
                val ticket = try {
                    store.rememberTap(message.record, message.notification.navigate, now)
                } catch (error: Exception) {
                    null
                }
                notifier.show(message.record, message.notification, ticket)
                PushRuntime.publishReceived(message.record)
            }
            is ReceivedMessage.Unreadable -> notifier.showGeneric(message.record)
            ReceivedMessage.StoreUnavailable -> notifier.showGeneric(null)
            ReceivedMessage.Unknown -> Unit
        }
    }

    private fun read(data: Map<String, String>): ReceivedMessage {
        val subscriptionId = data["s"] ?: return ReceivedMessage.Unknown
        val record = try {
            store.find(subscriptionId)
        } catch (error: Exception) {
            // A passing read failure: say that something arrived, nothing more.
            return ReceivedMessage.StoreUnavailable
        } ?: return ReceivedMessage.Unknown
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
