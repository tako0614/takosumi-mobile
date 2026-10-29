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

/** One app screen to show: identity, first URL and the name for its task. */
internal data class ScreenRequest(
    val key: String,
    val url: String,
    val title: String,
    val popup: Boolean = false,
)

internal object AppScreenLaunch {
    const val EXTRA_KEY = "com.takosumi.appscreen.KEY"
    const val EXTRA_URL = "com.takosumi.appscreen.URL"
    const val EXTRA_TITLE = "com.takosumi.appscreen.TITLE"
    const val EXTRA_POPUP = "com.takosumi.appscreen.POPUP"
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
        return ScreenRequest(key, url, title, popup)
    }

    /** Keep the screen's launch so its task can be restored without extras. */
    fun remember(context: Context, request: ScreenRequest) {
        if (request.popup || !AppScreenPolicy.isLaunchable(request.url)) return
        try {
            val preferences = context.getSharedPreferences(LAUNCH_PREFERENCES, Context.MODE_PRIVATE)
            val editor = preferences.edit().putString(
                digest(request.key),
                JSONObject()
                    .put("url", request.url)
                    .put("title", request.title)
                    .put("t", System.currentTimeMillis())
                    .toString(),
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
            ?.let { ScreenRequest(key, it.optString("url"), it.optString("title")) }
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
            val usage = JSONObject()
                .put("key", request.key)
                .put("url", request.url)
                .put("title", request.title)
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
                val screen = ScreenRequest(
                    item.optString("key"),
                    item.optString("url"),
                    item.optString("title"),
                )
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
            .setIntent(AppScreenLaunch.documentIntent(context, request))
            .setRank(rank)
            .build()
    }

    private fun shortcutId(key: String) = "screen-" + AppScreenLaunch.digest(key).take(40)
}
