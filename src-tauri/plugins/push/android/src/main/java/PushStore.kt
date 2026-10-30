package com.takosumi.push

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import java.io.File
import java.io.FileNotFoundException
import java.security.KeyStore
import java.security.SecureRandom
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** One Takosumi instance subscription, as the receiver needs it. */
internal class SubscriptionRecord(
    val subscriptionId: String,
    val instance: String,
    val account: String,
    val label: String,
    val origins: List<String>,
    val publicKey: ByteArray,
    val authSecret: ByteArray,
    /** PKCS#8 private key, encrypted with the Android Keystore wrapping key. */
    val wrappedPrivateKey: ByteArray,
)

/** A notification on screen, until it is tapped (or replaced by newer ones). */
internal class TapRecord(
    val token: String,
    val subscriptionId: String,
    val instance: String,
    val account: String,
    val navigate: String?,
    val createdAt: Long,
    /** When it was tapped (0 until then). */
    val tappedAt: Long = 0,
)

/** What a posted notification's tap intent carries, and its PendingIntent request code. */
internal class TapTicket(val token: String, val requestCode: Int)

/**
 * Subscriptions and taps, in one small JSON file under `noBackupFilesDir`
 * (never restored onto another device, where the wrapped keys would be
 * useless). Private keys are wrapped with an AES-GCM key that lives in the
 * Android Keystore and never leaves it; the file alone cannot decrypt a push.
 * The messaging service and the plugin share the process, so one lock is
 * enough.
 */
internal class PushStore(context: Context) {
    private val file = AtomicFile(File(context.noBackupFilesDir, FILE_NAME))

    fun list(): List<SubscriptionRecord> = synchronized(LOCK) {
        subscriptionsOf(read())
    }

    fun find(subscriptionId: String): SubscriptionRecord? = list().firstOrNull {
        it.subscriptionId == subscriptionId
    }

    fun create(
        subscriptionId: String,
        instance: String,
        account: String,
        label: String,
        origins: List<String>,
        keys: WebPushCrypto.KeyMaterial,
    ): SubscriptionRecord = synchronized(LOCK) {
        val root = read()
        val subscriptions = root.optJSONObject("subscriptions") ?: JSONObject()
        if (subscriptions.has(subscriptionId)) {
            throw IllegalStateException("subscription keys already exist")
        }
        if (subscriptions.length() >= MAX_SUBSCRIPTIONS) {
            throw IllegalStateException("too many subscriptions on this device")
        }
        val record = SubscriptionRecord(
            subscriptionId = subscriptionId,
            instance = instance,
            account = account,
            label = label,
            origins = origins,
            publicKey = keys.publicKey,
            authSecret = keys.authSecret,
            wrappedPrivateKey = KeyWrapper.wrap(keys.privateKeyPkcs8),
        )
        subscriptions.put(subscriptionId, encode(record))
        root.put("subscriptions", subscriptions)
        write(root)
        record
    }

    /** Removes the subscription and its notifications' taps; true when it existed. */
    fun delete(subscriptionId: String): Boolean = synchronized(LOCK) {
        val root = read()
        val subscriptions = root.optJSONObject("subscriptions") ?: return false
        if (!subscriptions.has(subscriptionId)) return false
        subscriptions.remove(subscriptionId)
        val taps = root.optJSONObject("taps")
        if (taps != null) {
            for (token in taps.keys().asSequence().toList()) {
                if (taps.optJSONObject(token)?.optString("subscriptionId") == subscriptionId) {
                    taps.remove(token)
                }
            }
        }
        write(root)
        true
    }

    fun privateKey(record: SubscriptionRecord): ByteArray = KeyWrapper.unwrap(record.wrappedPrivateKey)

    /**
     * Remembers what a posted notification opens (only notifications this
     * device could read: the subscription is then authentic) and returns the
     * opaque token its tap intent carries. An intent from anywhere else has no
     * known token and is ignored, so another app cannot fake a Takosumi
     * notification tap. Each ticket gets its own PendingIntent request code,
     * so one notification's PendingIntent never takes over another's extras.
     */
    fun rememberTap(record: SubscriptionRecord, navigate: String?, now: Long): TapTicket = synchronized(LOCK) {
        val root = read()
        val taps = root.optJSONObject("taps") ?: JSONObject()
        val token = randomToken()
        val entry = JSONObject()
            .put("subscriptionId", record.subscriptionId)
            .put("instance", record.instance)
            .put("account", record.account)
            .put("createdAt", now)
        if (navigate != null) entry.put("navigate", navigate)
        taps.put(token, entry)
        prune(taps, now)
        root.put("taps", taps)
        // Request codes count up (and wrap) across the store's life.
        val requestCode = root.optInt("nextRequestCode", 1).coerceAtLeast(1)
        root.put("nextRequestCode", if (requestCode == Int.MAX_VALUE) 1 else requestCode + 1)
        write(root)
        TapTicket(token, requestCode)
    }

    /** Takes the tap for `token` (once) and keeps it as the pending tap. */
    fun acceptTap(token: String, now: Long): TapRecord? = synchronized(LOCK) {
        val root = read()
        val taps = root.optJSONObject("taps") ?: return null
        val entry = taps.optJSONObject(token) ?: return null
        taps.remove(token)
        val tap = decodeTap(token, entry)
        if (tap == null || now - tap.createdAt > TAP_TTL_MILLIS) {
            write(root)
            return null
        }
        root.put("pendingTap", entry.put("token", token).put("tappedAt", now))
        write(root)
        tap
    }

    /**
     * The tap the shell has not handled yet (a cold start), at most once and
     * only for a few minutes after the tap: a tap the shell never took must not
     * switch accounts at some later launch.
     */
    fun takePendingTap(now: Long): TapRecord? = synchronized(LOCK) {
        val root = read()
        val entry = root.optJSONObject("pendingTap") ?: return null
        root.remove("pendingTap")
        write(root)
        val tap = decodeTap(entry.optString("token"), entry) ?: return null
        val fresh = now - tap.tappedAt in 0..PENDING_TAP_TTL_MILLIS && now - tap.createdAt <= TAP_TTL_MILLIS
        if (fresh) tap else null
    }

    private fun prune(taps: JSONObject, now: Long) {
        val entries = taps.keys().asSequence().toList().mapNotNull { token ->
            taps.optJSONObject(token)?.let { token to it.optLong("createdAt") }
        }
        for ((token, createdAt) in entries) {
            if (now - createdAt > TAP_TTL_MILLIS) taps.remove(token)
        }
        val remaining = entries.filter { taps.has(it.first) }.sortedBy { it.second }
        val excess = remaining.size - MAX_TAPS
        if (excess > 0) remaining.take(excess).forEach { taps.remove(it.first) }
    }

    /**
     * The stored state. No file yet is an empty state. A read that fails
     * (I/O) throws, so the caller writes nothing and a passing failure cannot
     * replace the subscriptions with an empty file. Only content that is not
     * JSON at all, which AtomicFile's whole-file replacement makes unlikely and
     * which holds nothing usable, starts over; the shell notices the missing
     * keys on its next check and subscribes again.
     */
    private fun read(): JSONObject {
        val bytes = try {
            file.readFully()
        } catch (error: FileNotFoundException) {
            return JSONObject().put("version", VERSION)
        }
        return try {
            JSONObject(String(bytes, Charsets.UTF_8))
        } catch (error: JSONException) {
            JSONObject().put("version", VERSION)
        }
    }

    private fun write(root: JSONObject) {
        root.put("version", VERSION)
        val stream = file.startWrite()
        try {
            stream.write(root.toString().toByteArray(Charsets.UTF_8))
            file.finishWrite(stream)
        } catch (error: Exception) {
            file.failWrite(stream)
            throw error
        }
    }

    private fun subscriptionsOf(root: JSONObject): List<SubscriptionRecord> {
        val subscriptions = root.optJSONObject("subscriptions") ?: return emptyList()
        return subscriptions.keys().asSequence().toList().mapNotNull { id ->
            subscriptions.optJSONObject(id)?.let { decode(id, it) }
        }
    }

    private fun encode(record: SubscriptionRecord): JSONObject =
        JSONObject()
            .put("instance", record.instance)
            .put("account", record.account)
            .put("label", record.label)
            .put("origins", JSONArray(record.origins))
            .put("publicKey", base64Url(record.publicKey))
            .put("auth", base64Url(record.authSecret))
            .put("wrappedPrivateKey", base64Url(record.wrappedPrivateKey))

    private fun decode(id: String, json: JSONObject): SubscriptionRecord? = try {
        val origins = json.getJSONArray("origins")
        SubscriptionRecord(
            subscriptionId = id,
            instance = json.getString("instance"),
            account = json.optString("account"),
            label = json.getString("label"),
            origins = (0 until origins.length()).map { origins.getString(it) },
            publicKey = fromBase64Url(json.getString("publicKey")),
            authSecret = fromBase64Url(json.getString("auth")),
            wrappedPrivateKey = fromBase64Url(json.getString("wrappedPrivateKey")),
        )
    } catch (error: Exception) {
        null
    }

    private fun decodeTap(token: String, json: JSONObject): TapRecord? {
        if (token.isEmpty()) return null
        return TapRecord(
            token = token,
            subscriptionId = json.optString("subscriptionId"),
            instance = json.optString("instance"),
            account = json.optString("account"),
            navigate = if (json.has("navigate")) json.optString("navigate") else null,
            createdAt = json.optLong("createdAt"),
            tappedAt = json.optLong("tappedAt"),
        )
    }

    companion object {
        private const val FILE_NAME = "takosumi-push.json"
        private const val VERSION = 1
        /** The relay allows 64 subscriptions per device. */
        private const val MAX_SUBSCRIPTIONS = 64
        private const val MAX_TAPS = 64
        private const val TAP_TTL_MILLIS = 30L * 24 * 60 * 60 * 1000
        /** How long a tap waits for the shell to take it (a cold start). */
        private const val PENDING_TAP_TTL_MILLIS = 5L * 60 * 1000
        private val LOCK = Any()
        private val random = SecureRandom()

        fun base64Url(bytes: ByteArray): String =
            Base64.getUrlEncoder().withoutPadding().encodeToString(bytes)

        fun fromBase64Url(text: String): ByteArray = Base64.getUrlDecoder().decode(text)

        private fun randomToken(): String {
            val bytes = ByteArray(16)
            random.nextBytes(bytes)
            return base64Url(bytes)
        }
    }
}

/** AES-256-GCM key in the Android Keystore that wraps the subscriptions' private keys. */
internal object KeyWrapper {
    private const val PROVIDER = "AndroidKeyStore"
    private const val ALIAS = "takosumi.push.wrap.v1"
    private const val IV_BYTES = 12

    fun wrap(plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val iv = cipher.iv
        if (iv.size != IV_BYTES) throw IllegalStateException("unexpected IV length")
        return iv + cipher.doFinal(plain)
    }

    fun unwrap(wrapped: ByteArray): ByteArray {
        if (wrapped.size <= IV_BYTES) throw IllegalStateException("wrapped key is truncated")
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, wrapped.copyOfRange(0, IV_BYTES)))
        return cipher.doFinal(wrapped.copyOfRange(IV_BYTES, wrapped.size))
    }

    @Synchronized
    private fun key(): SecretKey {
        val store = KeyStore.getInstance(PROVIDER).apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
        generator.init(
            KeyGenParameterSpec.Builder(
                ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                // Pushes arrive while the phone is locked: no user authentication.
                .setUserAuthenticationRequired(false)
                .build(),
        )
        return generator.generateKey()
    }
}
