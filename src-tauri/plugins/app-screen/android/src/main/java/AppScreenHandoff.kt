package com.takosumi.appscreen

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.browser.customtabs.CustomTabColorSchemeParams
import androidx.browser.customtabs.CustomTabsClient
import androidx.browser.customtabs.CustomTabsIntent
import org.json.JSONObject

/**
 * Hands a screen to its declared native app (`open_client`). The target app
 * is pinned with `setPackage`, so another app claiming the same scheme or
 * link never receives it; package visibility (`<queries>`,
 * `QUERY_ALL_PACKAGES`) is not needed and not used.
 */
internal object AppClientLauncher {
    const val OPENED = "opened"
    const val NOT_INSTALLED = "not_installed"

    /** [OPENED] when the app took the App Link or the custom-scheme link. */
    fun open(context: Context, packageName: String, appLink: String?, open: String?): String {
        if (!AppScreenRules.isAndroidPackage(packageName) ||
            packageName.equals(context.packageName, ignoreCase = true)
        ) {
            return NOT_INSTALLED
        }
        if (appLink != null && AppScreenRules.isHttpsLink(appLink)) {
            val intent = Intent(Intent.ACTION_VIEW, Uri.parse(appLink)).apply {
                setPackage(packageName)
                addCategory(Intent.CATEGORY_BROWSABLE)
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                // Only a non-browser app may take an https link (Android 11+).
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    addFlags(Intent.FLAG_ACTIVITY_REQUIRE_NON_BROWSER)
                }
            }
            if (start(context, intent)) return OPENED
        }
        if (open != null && AppScreenRules.isCustomSchemeLink(open)) {
            val intent = Intent(Intent.ACTION_VIEW, Uri.parse(open)).apply {
                setPackage(packageName)
                addCategory(Intent.CATEGORY_BROWSABLE)
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            if (start(context, intent)) return OPENED
        }
        return NOT_INSTALLED
    }

    private fun start(context: Context, intent: Intent): Boolean = try {
        context.startActivity(intent)
        true
    } catch (_: ActivityNotFoundException) {
        false
    } catch (_: SecurityException) {
        false
    }
}

/**
 * The browser view: a Custom Tab over Takosumi Mobile, in the person's own
 * browser with its sign-ins and saved passwords. External sign-ins (Google
 * forbids OAuth in embedded webviews) go here. Without a Custom Tabs browser
 * the page opens in the normal browser.
 */
internal object BrowserView {
    const val BROWSER_VIEW = "browser-view"
    const val BROWSER = "browser"

    /** Throws [ActivityNotFoundException] when no browser can open the page. */
    fun open(activity: Activity, url: String, toolbarColor: Int): String {
        require(AppScreenPolicy.isLaunchable(url)) { "Only https pages open in the browser view." }
        val uri = Uri.parse(url)
        val provider = try {
            CustomTabsClient.getPackageName(activity, null)
        } catch (_: Exception) {
            null
        }
        if (provider != null && provider != activity.packageName) {
            val tab = CustomTabsIntent.Builder()
                .setDefaultColorSchemeParams(
                    CustomTabColorSchemeParams.Builder().setToolbarColor(toolbarColor).build(),
                )
                .setShowTitle(true)
                .setShareState(CustomTabsIntent.SHARE_STATE_ON)
                .build()
            // The browser itself, not an app that claims the link.
            tab.intent.setPackage(provider)
            try {
                tab.launchUrl(activity, uri)
                return BROWSER_VIEW
            } catch (_: ActivityNotFoundException) {
                // Fall through to the normal browser.
            }
        }
        openInBrowser(activity, url)
        return BROWSER
    }

    /**
     * The phone's own browser (「ブラウザで開く」, where a web app can be added
     * to the home screen), never an app that claims the link as an App Link:
     * the intent resolves only among browsers (`CATEGORY_APP_BROWSER`
     * selector). Throws [ActivityNotFoundException] without a browser.
     */
    fun openInBrowser(activity: Activity, url: String) {
        require(AppScreenPolicy.isLaunchable(url)) { "Only https pages open in the browser." }
        val view = Intent(Intent.ACTION_VIEW, Uri.parse(url)).apply {
            addCategory(Intent.CATEGORY_BROWSABLE)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            selector = Intent.makeMainSelectorActivity(Intent.ACTION_MAIN, Intent.CATEGORY_APP_BROWSER)
        }
        activity.startActivity(view)
    }
}

/**
 * The last page of each app per (instance, account), so reopening an app
 * starts where the person left it. Only pages [AppScreenRules.isRememberable]
 * accepts are kept: the app's own origin, no credentials.
 */
internal object AppScreenResume {
    private const val PREFERENCES = "com.takosumi.appscreen.resume.v1"
    private const val MAX_REMEMBERED = 64

    fun record(context: Context, request: ScreenRequest, url: String) {
        val scope = request.scope ?: return
        val appOrigin = request.appOrigin ?: return
        if (request.popup || !AppScreenRules.isRememberable(url, appOrigin)) return
        try {
            val preferences = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            val id = id(scope, request.key)
            if (preferences.getString(id, null)?.let { JSONObject(it).optString("url") } == url) return
            val editor = preferences.edit()
                .putString(id, JSONObject().put("url", url).put("t", System.currentTimeMillis()).toString())
            val all = preferences.all
            if (all.size >= MAX_REMEMBERED) {
                all.mapNotNull { (storedId, value) ->
                    (value as? String)?.let { runCatching { storedId to JSONObject(it).optLong("t") }.getOrNull() }
                }
                    .filter { it.first != id }
                    .sortedBy { it.second }
                    .take(all.size - MAX_REMEMBERED + 1)
                    .forEach { editor.remove(it.first) }
            }
            editor.apply()
        } catch (_: Exception) {
            // Returning to the last page is a convenience.
        }
    }

    fun recall(context: Context, request: ScreenRequest): String? {
        val scope = request.scope ?: return null
        val appOrigin = request.appOrigin ?: return null
        return try {
            context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
                .getString(id(scope, request.key), null)
                ?.let { JSONObject(it).optString("url") }
                ?.takeIf { AppScreenRules.isRememberable(it, appOrigin) }
        } catch (_: Exception) {
            null
        }
    }

    private fun id(scope: String, key: String) = AppScreenLaunch.digest("$scope\n$key")
}

/** What an app screen's bar asks the shell to do: show the chooser for an app. */
internal data class PendingAction(val key: String, val url: String, val reason: String) {
    fun toJson(): JSONObject = JSONObject()
        .put("kind", "choose")
        .put("key", key)
        .put("url", url)
        .put("reason", reason)
}

/**
 * The bar's actions for the shell (開き方を変える, a native app that is not
 * installed, an unverified one to confirm). The action waits here until the
 * shell takes it (`take_pending_action`); the shell is told through the
 * plugin's `actionPending` event when it is running, and brought to the front.
 */
internal object AppScreenActions {
    const val CHANGE = "change"
    const val NOT_INSTALLED = "not-installed"
    const val UNVERIFIED = "unverified"

    private var pending: PendingAction? = null

    @Synchronized
    fun take(): PendingAction? = pending.also { pending = null }

    fun request(activity: Activity, action: PendingAction) {
        if (!AppScreenPolicy.isLaunchable(action.url)) return
        synchronized(this) { pending = action }
        AppScreenPlugin.notifyActionPending()
        val launch = activity.packageManager.getLaunchIntentForPackage(activity.packageName) ?: return
        launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        activity.startActivity(launch)
    }
}
