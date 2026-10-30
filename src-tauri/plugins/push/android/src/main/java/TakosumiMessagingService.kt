package com.takosumi.push

import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

/**
 * FCM entry point. Data messages from the relay carry only `{s, b}`; they are
 * decrypted and shown here even when the app is not running. A new token goes
 * to the shell, which sends it to the relay (`PUT /v1/devices/{id}`).
 */
class TakosumiMessagingService : FirebaseMessagingService() {
    // Registration tokens, as the relay's FCM sender (`message.token`) needs;
    // see PushPlugin.getToken.
    @Deprecated("Registration tokens until the relay sends to Firebase Installation IDs")
    @Suppress("DEPRECATION")
    override fun onNewToken(token: String) {
        super.onNewToken(token)
        PushRuntime.publishToken(token)
    }

    override fun onMessageReceived(message: RemoteMessage) {
        super.onMessageReceived(message)
        PushReceiver(this).handle(message.data)
    }
}
