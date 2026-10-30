package com.takosumi.push

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.webkit.WebView
import androidx.appcompat.app.AppCompatActivity
import androidx.core.app.NotificationManagerCompat
import app.tauri.PermissionState
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import java.lang.ref.WeakReference
import java.util.concurrent.atomic.AtomicBoolean

private const val NOTIFICATION_PERMISSION_ALIAS = "notifications"
private const val TOKEN_TIMEOUT_MILLIS = 20_000L
private const val MAX_LABEL_CHARS = 128
private const val MAX_ACCOUNT_CHARS = 512
private const val MAX_ORIGINS = 4
private val SUBSCRIPTION_ID = Regex("^[A-Za-z0-9_-]{8,128}$")
private val HTTPS_ORIGIN = Regex("^https://[a-z0-9.\\-]+(?::[0-9]{1,5})?$")
private val LOOPBACK_ORIGIN = Regex("^http://(?:localhost|127\\.0\\.0\\.1|\\[::1\\])(?::[0-9]{1,5})?$")

internal const val EVENT_NOTIFICATION_TAPPED = "notification-tapped"
internal const val EVENT_NOTIFICATION_RECEIVED = "notification-received"
internal const val EVENT_TOKEN_REFRESH = "token-refresh"

@InvokeArg
class CreateKeysArgs {
    var subscriptionId: String = ""
    var instance: String = ""
    var account: String = ""
    var label: String = ""
    var origins: Array<String> = arrayOf()
}

@InvokeArg
class SubscriptionArgs {
    var subscriptionId: String = ""
}

/** Hands events from the messaging service to the shell while it is running. */
internal object PushRuntime {
    @Volatile
    private var plugin: WeakReference<PushPlugin>? = null

    fun attach(next: PushPlugin) {
        plugin = WeakReference(next)
    }

    fun detach(current: PushPlugin) {
        if (plugin?.get() === current) plugin = null
    }

    fun publishToken(token: String) {
        plugin?.get()?.emit(EVENT_TOKEN_REFRESH, tokenPayload(token))
    }

    fun publishReceived(record: SubscriptionRecord) {
        plugin?.get()?.emit(
            EVENT_NOTIFICATION_RECEIVED,
            JSObject().apply {
                put("subscriptionId", record.subscriptionId)
                put("instance", record.instance)
                put("account", record.account)
            },
        )
    }

    fun tokenPayload(token: String): JSObject =
        JSObject().apply {
            put("token", token)
            put("provider", "fcm")
            put("environment", "production")
        }
}

/**
 * Takosumi Mobile's own notifications on Android: the FCM token, one Web Push
 * key pair per Takosumi instance subscription (private keys wrapped by the
 * Android Keystore, see [PushStore]) and the taps on posted notifications.
 * Decryption and display happen in [TakosumiMessagingService].
 */
@TauriPlugin(
    permissions = [
        Permission(
            strings = [Manifest.permission.POST_NOTIFICATIONS],
            alias = NOTIFICATION_PERMISSION_ALIAS,
        ),
    ],
)
class PushPlugin(private val activity: Activity) : Plugin(activity) {
    private val store by lazy { PushStore(activity.applicationContext) }
    private val notifier by lazy { PushNotifier(activity.applicationContext) }

    override fun load(webView: WebView) {
        super.load(webView)
        PushRuntime.attach(this)
        acceptTap(activity.intent)
    }

    override fun onNewIntent(intent: Intent) {
        acceptTap(intent)
    }

    override fun onDestroy(activity: AppCompatActivity) {
        PushRuntime.detach(this)
    }

    @Command
    fun permissionState(invoke: Invoke) {
        val state = when {
            !NotificationManagerCompat.from(activity).areNotificationsEnabled() &&
                Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU -> "denied"
            Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU -> "granted"
            else -> when (getPermissionState(NOTIFICATION_PERMISSION_ALIAS)) {
                PermissionState.GRANTED ->
                    if (NotificationManagerCompat.from(activity).areNotificationsEnabled()) "granted" else "denied"
                PermissionState.DENIED -> "denied"
                else -> "prompt"
            }
        }
        invoke.resolve(JSObject().apply { put("state", state) })
    }

    @Command
    fun requestPermission(invoke: Invoke) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            resolveGranted(invoke, NotificationManagerCompat.from(activity).areNotificationsEnabled())
            return
        }
        activity.runOnUiThread {
            requestPermissionForAlias(
                NOTIFICATION_PERMISSION_ALIAS,
                invoke,
                "requestPermissionCallback",
            )
        }
    }

    @PermissionCallback
    fun requestPermissionCallback(invoke: Invoke) {
        resolveGranted(invoke, getPermissionState(NOTIFICATION_PERMISSION_ALIAS) == PermissionState.GRANTED)
    }

    @Command
    fun getToken(invoke: Invoke) {
        // Without a Firebase configuration (google-services.json) in this build
        // there is no default app and no token: say so instead of crashing.
        if (FirebaseApp.getApps(activity.applicationContext).isEmpty()) {
            invoke.reject("push_unavailable: Firebase is not configured in this build", "push_unavailable")
            return
        }
        val messaging = try {
            FirebaseMessaging.getInstance()
        } catch (error: IllegalStateException) {
            invoke.reject("push_unavailable: Firebase is not configured in this build", "push_unavailable")
            return
        }
        messaging.isAutoInitEnabled = true
        val completed = AtomicBoolean(false)
        val handler = Handler(Looper.getMainLooper())
        val timeout = Runnable {
            if (completed.compareAndSet(false, true)) invoke.reject("FCM token request timed out.")
        }
        handler.postDelayed(timeout, TOKEN_TIMEOUT_MILLIS)
        // A registration token, not a Firebase Installation ID: the relay sends
        // FCM HTTP v1 `message.token`. firebase-messaging 25 deprecates
        // getToken() in favour of register()/`message.fid`; moving needs the
        // relay's FCM sender to change first.
        @Suppress("DEPRECATION")
        val tokenTask = messaging.token
        tokenTask.addOnCompleteListener { task ->
            if (!completed.compareAndSet(false, true)) return@addOnCompleteListener
            handler.removeCallbacks(timeout)
            val token = if (task.isSuccessful) task.result?.trim() else null
            if (token.isNullOrEmpty() || token.length > 1024) {
                invoke.reject("FCM token is unavailable.")
            } else {
                invoke.resolve(PushRuntime.tokenPayload(token))
            }
        }
    }

    @Command
    fun deviceName(invoke: Invoke) {
        val manufacturer = Build.MANUFACTURER.orEmpty().trim()
        val model = Build.MODEL.orEmpty().trim()
        val name = when {
            model.isEmpty() -> "Android"
            manufacturer.isEmpty() || model.startsWith(manufacturer, ignoreCase = true) -> model
            else -> "${manufacturer.replaceFirstChar { it.uppercaseChar() }} $model"
        }
        invoke.resolve(JSObject().apply { put("name", name.take(MAX_LABEL_CHARS)) })
    }

    @Command
    fun createSubscriptionKeys(invoke: Invoke) {
        val args = invoke.parseArgs(CreateKeysArgs::class.java)
        val origins = args.origins.toList()
        val label = args.label.trim()
        val invalid = when {
            !SUBSCRIPTION_ID.matches(args.subscriptionId) -> "subscriptionId is invalid"
            !isOrigin(args.instance) -> "instance must be an https origin"
            args.account.length > MAX_ACCOUNT_CHARS || args.account.any { it.isISOControl() } -> "account is invalid"
            label.isEmpty() || label.length > MAX_LABEL_CHARS || label.any { it.isISOControl() } -> "label is invalid"
            origins.isEmpty() || origins.size > MAX_ORIGINS || !origins.all(::isOrigin) -> "origins are invalid"
            args.instance !in origins -> "origins must include the instance"
            else -> null
        }
        if (invalid != null) {
            invoke.reject(invalid)
            return
        }
        try {
            val record = store.create(
                subscriptionId = args.subscriptionId,
                instance = args.instance,
                account = args.account,
                label = label,
                origins = origins,
                keys = WebPushCrypto.generate(),
            )
            notifier.ensureChannel(record)
            invoke.resolve(
                JSObject().apply {
                    put("p256dh", PushStore.base64Url(record.publicKey))
                    put("auth", PushStore.base64Url(record.authSecret))
                },
            )
        } catch (error: Exception) {
            invoke.reject("Could not create subscription keys: ${error.message}")
        }
    }

    @Command
    fun deleteSubscriptionKeys(invoke: Invoke) {
        val args = invoke.parseArgs(SubscriptionArgs::class.java)
        if (!SUBSCRIPTION_ID.matches(args.subscriptionId)) {
            invoke.reject("subscriptionId is invalid")
            return
        }
        val record = store.find(args.subscriptionId)
        store.delete(args.subscriptionId)
        // The channel goes with its last subscription for that instance/account.
        if (record != null && store.list().none { PushNotifier.channelId(it) == PushNotifier.channelId(record) }) {
            notifier.deleteChannel(record)
        }
        invoke.resolve()
    }

    @Command
    fun listSubscriptions(invoke: Invoke) {
        val list = JSArray()
        for (record in store.list()) {
            list.put(
                JSObject().apply {
                    put("subscriptionId", record.subscriptionId)
                    put("instance", record.instance)
                    put("account", record.account)
                    put("label", record.label)
                },
            )
        }
        invoke.resolve(JSObject().apply { put("subscriptions", list) })
    }

    @Command
    fun takePendingTap(invoke: Invoke) {
        val tap = store.takePendingTap(System.currentTimeMillis())
        invoke.resolve(JSObject().apply { if (tap != null) put("tap", tapPayload(tap)) })
    }

    internal fun emit(event: String, payload: JSObject) {
        trigger(event, payload)
    }

    /**
     * A tap on one of our notifications: the intent carries only an opaque
     * token, looked up in [PushStore]. The tap becomes the pending tap (a cold
     * start reads it with take_pending_tap) and an event for a running shell.
     */
    private fun acceptTap(intent: Intent?) {
        val token = intent?.getStringExtra(EXTRA_TAP_TOKEN) ?: return
        // Recreating the activity or reopening it from Recents re-delivers the
        // same intent; the token is spent either way.
        intent.removeExtra(EXTRA_TAP_TOKEN)
        val tap = store.acceptTap(token, System.currentTimeMillis()) ?: return
        trigger(EVENT_NOTIFICATION_TAPPED, tapPayload(tap))
    }

    private fun tapPayload(tap: TapRecord): JSObject =
        JSObject().apply {
            put("id", tap.token)
            put("subscriptionId", tap.subscriptionId)
            put("instance", tap.instance)
            put("account", tap.account)
            if (tap.navigate != null) put("navigate", tap.navigate)
        }

    private fun resolveGranted(invoke: Invoke, granted: Boolean) {
        invoke.resolve(JSObject().apply { put("granted", granted) })
    }

    private fun isOrigin(value: String): Boolean =
        value.length <= 256 && (HTTPS_ORIGIN.matches(value) || LOOPBACK_ORIGIN.matches(value))
}
