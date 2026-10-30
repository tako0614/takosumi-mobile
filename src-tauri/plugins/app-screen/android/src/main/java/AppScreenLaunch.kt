package com.takosumi.appscreen

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Rect
import android.net.Uri
import android.util.Base64
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.nio.charset.StandardCharsets
import java.security.MessageDigest

/**
 * The declared native app the bar offers ("{name}で開く"). A verified one has
 * its links; an unverified one has none and sends the person to the chooser.
 */
internal data class BannerClient(
    val name: String,
    val verified: Boolean,
    val packageName: String? = null,
    val appLink: String? = null,
    val open: String? = null,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("name", name)
        .put("verified", verified)
        .apply {
            packageName?.let { put("package", it) }
            appLink?.let { put("appLink", it) }
            open?.let { put("open", it) }
        }

    companion object {
        /** Re-checks everything; an invalid client is dropped, not repaired. */
        fun of(
            name: String?,
            verified: Boolean,
            packageName: String?,
            appLink: String?,
            open: String?,
        ): BannerClient? {
            if (!AppScreenRules.isClientName(name)) return null
            if (!verified) {
                return if (packageName == null && appLink == null && open == null) {
                    BannerClient(name!!.trim(), verified = false)
                } else {
                    null
                }
            }
            if (!AppScreenRules.isAndroidPackage(packageName)) return null
            if (appLink != null && !AppScreenRules.isHttpsLink(appLink)) return null
            if (open != null && !AppScreenRules.isCustomSchemeLink(open)) return null
            if (appLink == null && open == null) return null
            return BannerClient(name!!.trim(), verified = true, packageName, appLink, open)
        }

        fun fromJson(raw: String?): BannerClient? = try {
            raw?.let { JSONObject(it) }?.let { json ->
                of(
                    json.optString("name").ifEmpty { null },
                    json.optBoolean("verified", false),
                    json.optString("package").ifEmpty { null },
                    json.optString("appLink").ifEmpty { null },
                    json.optString("open").ifEmpty { null },
                )
            }
        } catch (_: Exception) {
            null
        }
    }
}

/** One app screen to show: identity, first URL and the name for its task. */
internal data class ScreenRequest(
    val key: String,
    val url: String,
    val title: String,
    val popup: Boolean = false,
    /** The instance's Accounts origin, kept in the screen with the app's own. */
    val accountsOrigin: String? = null,
    /** (instance, account) the last page is remembered for. */
    val scope: String? = null,
    /** Reopen at the last remembered page instead of [url]. */
    val resume: Boolean = false,
    val client: BannerClient? = null,
) {
    val appOrigin: String? get() = AppScreenRules.originOf(url)

    /** The origins this screen shows in its main frame; others go to the browser view. */
    val allowedOrigins: Set<String>
        get() = setOfNotNull(appOrigin, accountsOrigin)

    /**
     * What is kept beyond this launch (task restore, shortcuts): the declared
     * app only as a label, never its links, so an app the operator stops
     * trusting is not opened from a stale copy; the bar then asks the shell,
     * which reads the current declaration.
     */
    fun persisted(): ScreenRequest = copy(client = client?.let { BannerClient(it.name, verified = false) })

    fun toJson(): JSONObject = persisted().let { kept ->
        JSONObject()
            .put("url", kept.url)
            .put("title", kept.title)
            .apply {
                kept.accountsOrigin?.let { put("accountsOrigin", it) }
                kept.scope?.let { put("scope", it) }
                kept.client?.let { put("client", it.toJson().toString()) }
            }
    }

    companion object {
        const val SCOPE_MAX_LENGTH = 512

        fun checkedAccountsOrigin(raw: String?): String? =
            raw?.takeIf { AppScreenRules.isHttpsOrigin(it) }

        fun checkedScope(raw: String?): String? =
            raw?.takeIf { it.isNotEmpty() && it.length <= SCOPE_MAX_LENGTH && it.none { c -> c.isISOControl() } }

        /** A request stored by [toJson] (task restore, shortcuts). */
        fun fromJson(key: String, json: JSONObject): ScreenRequest = ScreenRequest(
            key = key,
            url = json.optString("url"),
            title = json.optString("title"),
            accountsOrigin = checkedAccountsOrigin(json.optString("accountsOrigin").ifEmpty { null }),
            scope = checkedScope(json.optString("scope").ifEmpty { null }),
            resume = true,
            client = BannerClient.fromJson(json.optString("client").ifEmpty { null }),
        )
    }
}

internal object AppScreenLaunch {
    const val EXTRA_KEY = "com.takosumi.appscreen.KEY"
    const val EXTRA_URL = "com.takosumi.appscreen.URL"
    const val EXTRA_TITLE = "com.takosumi.appscreen.TITLE"
    const val EXTRA_POPUP = "com.takosumi.appscreen.POPUP"
    const val EXTRA_ACCOUNTS_ORIGIN = "com.takosumi.appscreen.ACCOUNTS_ORIGIN"
    const val EXTRA_SCOPE = "com.takosumi.appscreen.SCOPE"
    const val EXTRA_RESUME = "com.takosumi.appscreen.RESUME"
    const val EXTRA_CLIENT = "com.takosumi.appscreen.CLIENT"
    private const val SCHEME = "takosumi"
    private const val HOST = "app"
    private const val KEY_MAX_LENGTH = 512
    private const val LAUNCH_PREFERENCES = "com.takosumi.appscreen.launch.v1"
    private const val MAX_REMEMBERED = 64
    private const val TITLE_MAX_LENGTH = 128

    /**
     * The intent for an app's own task. Its data, `takosumi://app?key=...`,
     * names the document, so the same app returns to its existing task.
     */
    fun documentIntent(context: Context, request: ScreenRequest): Intent =
        Intent(context, AppScreenActivity::class.java).apply {
            action = Intent.ACTION_VIEW
            data = documentUri(request.key)
            putExtra(EXTRA_KEY, request.key)
            putExtra(EXTRA_URL, request.url)
            putExtra(EXTRA_TITLE, request.title)
            putExtra(EXTRA_RESUME, request.resume)
            request.accountsOrigin?.let { putExtra(EXTRA_ACCOUNTS_ORIGIN, it) }
            request.scope?.let { putExtra(EXTRA_SCOPE, it) }
            request.client?.let { putExtra(EXTRA_CLIENT, it.toJson().toString()) }
            addFlags(Intent.FLAG_ACTIVITY_NEW_DOCUMENT)
        }

    /** A `window.open` / `target=_blank` screen, stacked on its opener. */
    fun popupIntent(context: Context, key: String, title: String): Intent =
        Intent(context, AppScreenActivity::class.java).apply {
            action = Intent.ACTION_VIEW
            data = documentUri(key)
            putExtra(EXTRA_KEY, key)
            putExtra(EXTRA_URL, "about:blank")
            putExtra(EXTRA_TITLE, title)
            putExtra(EXTRA_POPUP, true)
        }

    fun documentUri(key: String): Uri =
        Uri.Builder().scheme(SCHEME).authority(HOST).appendQueryParameter("key", key).build()

    /**
     * Reads a screen intent. Accepts the extras written above; a document
     * restored without extras (Recents after a reboot) uses the launch stored
     * for its key; a `takosumi://app?url=` link may carry the URL alone
     * (keyed by its origin).
     */
    fun parse(context: Context, intent: Intent?): ScreenRequest? {
        if (intent == null) return null
        val popup = intent.getBooleanExtra(EXTRA_POPUP, false)
        val data = intent.data?.takeIf { it.scheme == SCHEME && it.host == HOST }
        val dataKey = data?.getQueryParameter("key")
        val stored = if (!popup && intent.getStringExtra(EXTRA_URL) == null && dataKey != null) {
            recall(context, dataKey)
        } else {
            null
        }
        val url = intent.getStringExtra(EXTRA_URL)
            ?: stored?.url
            ?: data?.getQueryParameter("url")
            ?: return null
        if (!popup && !AppScreenPolicy.isLaunchable(url)) return null
        val key = intent.getStringExtra(EXTRA_KEY)
            ?: dataKey
            ?: AppScreenPolicy.origin(url)?.let { "origin:$it" }
            ?: return null
        if (key.isBlank() || key.length > KEY_MAX_LENGTH) return null
        val title = (intent.getStringExtra(EXTRA_TITLE) ?: stored?.title ?: AppScreenPolicy.displayHost(url))
            .take(TITLE_MAX_LENGTH)
        if (popup) return ScreenRequest(key, url, title, popup = true)
        if (stored != null) return stored.copy(key = key, title = title)
        return ScreenRequest(
            key = key,
            url = url,
            title = title,
            accountsOrigin = ScreenRequest.checkedAccountsOrigin(intent.getStringExtra(EXTRA_ACCOUNTS_ORIGIN)),
            scope = ScreenRequest.checkedScope(intent.getStringExtra(EXTRA_SCOPE)),
            // A bare `takosumi://app?url=` names a page; it never resumes elsewhere.
            resume = intent.getBooleanExtra(EXTRA_RESUME, false),
            client = BannerClient.fromJson(intent.getStringExtra(EXTRA_CLIENT)),
        )
    }

    /** Keep the screen's launch so its task can be restored without extras. */
    fun remember(context: Context, request: ScreenRequest) {
        if (request.popup || !AppScreenPolicy.isLaunchable(request.url)) return
        try {
            val preferences = context.getSharedPreferences(LAUNCH_PREFERENCES, Context.MODE_PRIVATE)
            val editor = preferences.edit().putString(
                digest(request.key),
                request.toJson().put("t", System.currentTimeMillis()).toString(),
            )
            val all = preferences.all
            if (all.size > MAX_REMEMBERED) {
                all.mapNotNull { (id, value) ->
                    (value as? String)?.let { runCatching { id to JSONObject(it).optLong("t") }.getOrNull() }
                }
                    .sortedBy { it.second }
                    .take(all.size - MAX_REMEMBERED)
                    .forEach { editor.remove(it.first) }
            }
            editor.apply()
        } catch (_: Exception) {
            // Restoring after a reboot is a convenience.
        }
    }

    private fun recall(context: Context, key: String): ScreenRequest? = try {
        context.getSharedPreferences(LAUNCH_PREFERENCES, Context.MODE_PRIVATE)
            .getString(digest(key), null)
            ?.let { JSONObject(it) }
            ?.let { ScreenRequest.fromJson(key, it) }
            ?.takeIf { AppScreenPolicy.isLaunchable(it.url) }
    } catch (_: Exception) {
        null
    }

    fun digest(key: String): String =
        MessageDigest.getInstance("SHA-256")
            .digest(key.toByteArray(StandardCharsets.UTF_8))
            .joinToString("") { byte -> "%02x".format(byte.toInt() and 0xff) }
}

/**
 * Icons drawn by the shell (PNG, already rasterized from SVG or a glyph),
 * stored per screen so tasks and shortcuts have them even when the shell is
 * not running.
 */
internal object AppScreenIcons {
    private const val DIRECTORY = "app-screen-icons"
    private const val MAX_PIXELS = 256

    fun store(context: Context, key: String, base64Png: String?) {
        if (base64Png.isNullOrEmpty()) return
        val bytes = try {
            Base64.decode(base64Png, Base64.DEFAULT)
        } catch (_: IllegalArgumentException) {
            return
        }
        // Decode and re-encode, so only a real bounded bitmap is kept.
        val decoded = BitmapFactory.decodeByteArray(bytes, 0, bytes.size) ?: return
        val bitmap = scaled(decoded, MAX_PIXELS)
        val file = file(context, key)
        file.parentFile?.mkdirs()
        val temp = File(file.parentFile, "${file.name}.tmp")
        FileOutputStream(temp).use { out -> bitmap.compress(Bitmap.CompressFormat.PNG, 100, out) }
        if (!temp.renameTo(file)) temp.delete()
    }

    fun load(context: Context, key: String): Bitmap? {
        val file = file(context, key)
        return if (file.isFile) BitmapFactory.decodeFile(file.absolutePath) else null
    }

    fun delete(context: Context, key: String) {
        file(context, key).delete()
    }

    /** Launcher masks crop adaptive icons; keep the icon inside the safe zone. */
    fun adaptive(icon: Bitmap): Bitmap {
        val size = 216
        val inner = 144
        val output = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(output)
        canvas.drawColor(Color.WHITE)
        val offset = (size - inner) / 2
        canvas.drawBitmap(
            icon,
            null,
            Rect(offset, offset, offset + inner, offset + inner),
            Paint(Paint.FILTER_BITMAP_FLAG or Paint.ANTI_ALIAS_FLAG),
        )
        return output
    }

    private fun scaled(bitmap: Bitmap, max: Int): Bitmap {
        val largest = maxOf(bitmap.width, bitmap.height)
        if (largest <= max) return bitmap
        val ratio = max.toFloat() / largest
        return Bitmap.createScaledBitmap(
            bitmap,
            maxOf(1, (bitmap.width * ratio).toInt()),
            maxOf(1, (bitmap.height * ratio).toInt()),
            true,
        )
    }

    private fun file(context: Context, key: String) =
        File(File(context.filesDir, DIRECTORY), "${AppScreenLaunch.digest(key)}.png")
}

/**
 * Home-screen shortcuts: pinned ones the person adds from an app screen, and
 * dynamic ones (the long-press menu) for the most used apps.
 */
internal object AppScreenShortcuts {
    private const val PREFERENCES = "com.takosumi.appscreen.usage.v1"
    private const val MAX_TRACKED = 32

    fun isPinSupported(context: Context): Boolean =
        ShortcutManagerCompat.isRequestPinShortcutSupported(context)

    fun requestPin(context: Context, request: ScreenRequest): Boolean {
        if (!isPinSupported(context)) return false
        return ShortcutManagerCompat.requestPinShortcut(context, info(context, request), null)
    }

    /** Count an open and keep the most used apps as dynamic shortcuts. */
    fun reportOpened(context: Context, request: ScreenRequest) {
        if (request.popup) return
        try {
            val preferences = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            val id = shortcutId(request.key)
            val previous = preferences.getString(id, null)?.let { runCatching { JSONObject(it) }.getOrNull() }
            val usage = request.toJson()
                .put("key", request.key)
                .put("count", (previous?.optInt("count") ?: 0) + 1)
                .put("lastUsed", System.currentTimeMillis())
            val editor = preferences.edit().putString(id, usage.toString())
            val all = preferences.all.mapNotNull { (storedId, value) ->
                if (storedId == id) null
                else (value as? String)?.let { runCatching { JSONObject(it) }.getOrNull() }
            } + usage
            all.sortedBy { it.optLong("lastUsed") }
                .take(maxOf(0, all.size - MAX_TRACKED))
                .forEach { stale -> editor.remove(shortcutId(stale.optString("key"))) }
            editor.apply()

            val limit = minOf(4, ShortcutManagerCompat.getMaxShortcutCountPerActivity(context))
            val top = all.sortedWith(
                compareByDescending<JSONObject> { it.optInt("count") }
                    .thenByDescending { it.optLong("lastUsed") },
            ).take(limit)
            val shortcuts = top.mapIndexedNotNull { rank, item ->
                val screen = ScreenRequest.fromJson(item.optString("key"), item)
                if (screen.key.isEmpty() || !AppScreenPolicy.isLaunchable(screen.url)) null
                else info(context, screen, rank)
            }
            ShortcutManagerCompat.setDynamicShortcuts(context, shortcuts)
            ShortcutManagerCompat.reportShortcutUsed(context, id)
        } catch (_: Exception) {
            // Shortcuts are a convenience; opening the screen must not fail.
        }
    }

    fun info(context: Context, request: ScreenRequest, rank: Int = 0): ShortcutInfoCompat {
        val icon = AppScreenIcons.load(context, request.key)
            ?.let { IconCompat.createWithAdaptiveBitmap(AppScreenIcons.adaptive(it)) }
            ?: IconCompat.createWithResource(context, context.applicationInfo.icon)
        val label = request.title.ifBlank { AppScreenPolicy.displayHost(request.url) }
        return ShortcutInfoCompat.Builder(context, shortcutId(request.key))
            .setShortLabel(label.take(25))
            .setLongLabel(label.take(64))
            .setIcon(icon)
            .setIntent(AppScreenLaunch.documentIntent(context, request.persisted()))
            .setRank(rank)
            .build()
    }

    private fun shortcutId(key: String) = "screen-" + AppScreenLaunch.digest(key).take(40)
}
