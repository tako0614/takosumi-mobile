package com.takosumi.push

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Locale

/** The extra a Takosumi notification's tap intent carries: an opaque token. */
internal const val EXTRA_TAP_TOKEN = "com.takosumi.push.TAP_TOKEN"

/**
 * Shows Takosumi notifications: one channel per (instance, account), named
 * after it, so the person can silence one Takosumi without the others, and a
 * general channel for messages this device cannot attribute or read.
 */
internal class PushNotifier(private val context: Context) {
    private val manager = NotificationManagerCompat.from(context)

    fun ensureChannel(record: SubscriptionRecord) {
        val channel = NotificationChannel(
            channelId(record),
            record.label,
            NotificationManager.IMPORTANCE_HIGH,
        )
        channel.description = text("通知: ${record.label}", "Notifications from ${record.label}")
        context.getSystemService(NotificationManager::class.java)?.createNotificationChannel(channel)
    }

    fun deleteChannel(record: SubscriptionRecord) {
        context.getSystemService(NotificationManager::class.java)
            ?.deleteNotificationChannel(channelId(record))
    }

    fun show(record: SubscriptionRecord, notification: DeclarativeNotification, tapToken: String) {
        ensureChannel(record)
        val channel = channelId(record)
        post(
            channel = channel,
            // The same `tag` replaces the earlier notification (one Run, one app).
            tag = "$channel:${notification.tag ?: randomTag()}",
            title = notification.title,
            body = notification.body,
            tapToken = tapToken,
        )
    }

    /**
     * A message this device cannot attribute (unknown `s`) or read: generic
     * text, never the instance's words. A known subscription still opens its
     * instance when tapped.
     */
    fun showGeneric(record: SubscriptionRecord?, tapToken: String) {
        val channel = if (record != null) {
            ensureChannel(record)
            channelId(record)
        } else {
            ensureGeneralChannel()
            GENERAL_CHANNEL
        }
        post(channel, "$channel:${randomTag()}", "Takosumi", genericBody(), tapToken)
    }

    private fun post(channel: String, tag: String, title: String, body: String?, tapToken: String) {
        if (!canPost()) return
        val launch = context.packageManager.getLaunchIntentForPackage(context.packageName) ?: return
        launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        launch.putExtra(EXTRA_TAP_TOKEN, tapToken)
        val pending = PendingIntent.getActivity(
            context,
            tapToken.hashCode(),
            launch,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val publicVersion = NotificationCompat.Builder(context, channel)
            .setSmallIcon(R.drawable.takosumi_push_icon)
            .setContentTitle("Takosumi")
            .setContentText(genericBody())
            .build()
        val builder = NotificationCompat.Builder(context, channel)
            .setSmallIcon(R.drawable.takosumi_push_icon)
            .setContentTitle(title)
            .setAutoCancel(true)
            .setContentIntent(pending)
            .setShowWhen(true)
            .setWhen(System.currentTimeMillis())
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(publicVersion)
        if (body != null) {
            builder.setContentText(body)
            builder.setStyle(NotificationCompat.BigTextStyle().bigText(body))
        }
        try {
            manager.notify(tag, NOTIFICATION_ID, builder.build())
        } catch (error: SecurityException) {
            // The permission was withdrawn between the check and the post.
        }
    }

    private fun canPost(): Boolean {
        if (!manager.areNotificationsEnabled()) return false
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return true
        return ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
    }

    private fun ensureGeneralChannel() {
        val channel = NotificationChannel(
            GENERAL_CHANNEL,
            "Takosumi",
            NotificationManager.IMPORTANCE_DEFAULT,
        )
        context.getSystemService(NotificationManager::class.java)?.createNotificationChannel(channel)
    }

    private fun genericBody(): String = text("新しい通知があります", "You have a new notification")

    private fun text(ja: String, en: String): String =
        if (Locale.getDefault().language == "ja") ja else en

    companion object {
        private const val GENERAL_CHANNEL = "takosumi.general"
        private const val NOTIFICATION_ID = 1
        private val random = SecureRandom()

        fun channelId(record: SubscriptionRecord): String {
            val digest = MessageDigest.getInstance("SHA-256")
                .digest("${record.instance}\u0000${record.account}".toByteArray(Charsets.UTF_8))
            val hex = digest.take(12).joinToString("") { "%02x".format(it.toInt() and 0xff) }
            return "takosumi.instance.$hex"
        }

        private fun randomTag(): String = Integer.toHexString(random.nextInt())
    }
}
