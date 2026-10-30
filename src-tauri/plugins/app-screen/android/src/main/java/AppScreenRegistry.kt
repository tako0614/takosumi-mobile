package com.takosumi.appscreen

import android.annotation.SuppressLint
import android.content.Context
import android.content.MutableContextWrapper
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.WebSettings
import android.webkit.WebView

/**
 * Process-wide app screen webviews. One app screen stays alive at a time
 * (with the popups it opened): opening another app closes the previous
 * one's webview, and its activity reopens its last page by URL when it comes
 * back. Keeping several live webviews and switching between them waits for
 * device testing (TASK-0119 step 3).
 *
 * Every webview uses the process's single WebView profile (one cookie jar and
 * storage area), so a Takosumi sign-in made in one app screen carries over to
 * the others. Do not give screens their own data directory or profile.
 *
 * The webviews are plain `android.webkit.WebView`s, not Tauri webviews: no
 * JavaScript interface, no web message listener and no Tauri IPC, so a page
 * can reach neither native plugins nor the instance's tokens.
 *
 * Each webview lives in a [MutableContextWrapper]: it points at the activity
 * that shows it (dialogs, pickers) and back at the application while detached.
 */
internal object AppScreenRegistry {
    /** Live app screens, not counting the popups each one opened. */
    const val MAX_ALIVE_SCREENS = 1

    class Entry(
        val key: String,
        val webView: WebView,
        val context: MutableContextWrapper,
        val popup: Boolean,
    ) {
        /** URL the screen was opened with; reopening with it keeps the state. */
        var launchUrl: String = ""
        var lastUrl: String = ""
        var title: String = ""
        var host: AppScreenActivity? = null
        /** For a popup: the screen whose page opened it. */
        var openerKey: String? = null
        /** For a popup: its first main-frame navigation has been judged. */
        var popupTargetDecided = false
        val createdAt: Long = android.os.SystemClock.elapsedRealtime()
        var destroyed = false
        /** (instance, account) the screen was opened for. */
        var scope: String? = null

        /**
         * What the main frame may show (the app's origin, the Takosumi
         * origins' sign-in pages); the rest goes to the browser view. Read on
         * WebView threads.
         */
        @Volatile
        var origins: ScreenOrigins? = null

        /** The policy while no activity shows this webview. */
        val detachedClient = DetachedScreenClient(this)
    }

    // Access-ordered: iteration starts at the least recently used entry.
    private val entries = LinkedHashMap<String, Entry>(8, 0.75f, true)

    fun get(key: String): Entry? = entries[key]?.takeUnless { it.destroyed }

    fun obtain(context: Context, key: String, popup: Boolean = false): Entry {
        get(key)?.let { return it }
        val wrapper = MutableContextWrapper(context.applicationContext)
        val entry = Entry(key, createWebView(wrapper), wrapper, popup)
        // A webview without a client hands navigations to other apps; keep the
        // policy from the first navigation (a popup loads before its activity).
        entry.webView.webViewClient = entry.detachedClient
        entries[key] = entry
        if (!popup) evictOtherScreens(key)
        return entry
    }

    fun remove(key: String) {
        val entry = entries.remove(key) ?: return
        destroy(entry, crashed = false)
    }

    /** A webview whose renderer died may only be destroyed, never used. */
    fun removeCrashed(key: String) {
        val entry = entries.remove(key) ?: return
        destroy(entry, crashed = true)
    }

    fun findByWebView(webView: WebView): Entry? =
        entries.values.firstOrNull { it.webView === webView && !it.destroyed }

    /** The activity showing the screen that opened popup [popupKey], if any. */
    fun openerHost(popupKey: String): AppScreenActivity? =
        get(popupKey)?.openerKey?.let { get(it)?.host }

    /** Renderer gone for a webview no activity is handling: drop it safely. */
    fun handleCrashedWebView(webView: WebView) {
        val entry = findByWebView(webView)
        if (entry == null) {
            (webView.parent as? ViewGroup)?.removeView(webView)
            webView.destroy()
            return
        }
        entry.host?.releaseEvictedWebView(entry)
        removeCrashed(entry.key)
    }

    /**
     * Close every other app screen that is not visible (split screen keeps a
     * visible one), with the popups it opened. Their activities reload their
     * last page when they come back.
     */
    private fun evictOtherScreens(keepKey: String) {
        // MAX_ALIVE_SCREENS: only [keepKey] stays, unless another is on screen.
        val victims = entries.values.filter { entry ->
            !entry.popup && entry.key != keepKey && entry.host?.isVisibleScreen() != true
        }
        for (victim in victims) {
            for (entry in withPopups(victim)) {
                entries.remove(entry.key)
                entry.host?.releaseEvictedWebView(entry)
                destroy(entry, crashed = false)
            }
        }
    }

    /** [screen]'s popups (and theirs), innermost first, then [screen]. */
    private fun withPopups(screen: Entry): List<Entry> {
        val children = entries.values.filter { it.popup && it.openerKey == screen.key }
        return children.flatMap { withPopups(it) } + screen
    }

    private fun destroy(entry: Entry, crashed: Boolean) {
        if (entry.destroyed) return
        entry.destroyed = true
        val webView = entry.webView
        (webView.parent as? ViewGroup)?.removeView(webView)
        entry.host = null
        entry.context.baseContext = entry.context.applicationContext
        if (!crashed) {
            webView.stopLoading()
            webView.loadUrl("about:blank")
        }
        webView.destroy()
    }

    @SuppressLint("SetJavaScriptEnabled")
    private fun createWebView(context: MutableContextWrapper): WebView {
        val webView = WebView(context)
        with(webView.settings) {
            javaScriptEnabled = true
            domStorageEnabled = true
            setSupportMultipleWindows(true)
            // Popups need a user gesture, as in a browser.
            javaScriptCanOpenWindowsAutomatically = false
            allowFileAccess = false
            allowContentAccess = false
            mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
            mediaPlaybackRequiresUserGesture = true
            setGeolocationEnabled(false)
            setSupportZoom(true)
            builtInZoomControls = true
            displayZoomControls = false
            safeBrowsingEnabled = true
        }
        val cookies = CookieManager.getInstance()
        cookies.setAcceptCookie(true)
        cookies.setAcceptThirdPartyCookies(webView, false)
        return webView
    }
}
